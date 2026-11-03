//! Shared support for the integration tests: temporary folders, the repository's fake game
//! (`tests/fake_game.py`, started as a child of the binary under test and driven through two FIFOs), and runs
//! of the test build of the binary (feature `test-source`) with a control file and a per-poll trace.
//!
//! Only processes started here are ever looked at: the binary's test source lists nothing but the fake game,
//! and the fake game is always a descendant of the binary and of the test.
#![allow(dead_code)]

use dehulud::json::{self, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const EXE: &str = "DuneSandbox-Win64-Shipping.exe";
pub const WAIT: Duration = Duration::from_secs(20);

pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dehulud"))
}

/// The DeHulud Combat binary of this build.
#[allow(dead_code)]
pub fn combat_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dehulud-combat"))
}

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// The Python that runs the fake game: $DEHULUD_PYTHON, else /usr/bin/python3, else python3 from PATH.
pub fn python() -> String {
    std::env::var("DEHULUD_PYTHON").unwrap_or_else(|_| {
        if Path::new("/usr/bin/python3").exists() { "/usr/bin/python3".into() } else { "python3".into() }
    })
}

/// A folder under $TMPDIR, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "dehulud-it-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mkfifo(p: &Path) {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo {}", p.display());
}

/// Lines from a reader, delivered through a channel by a thread.
fn line_channel(r: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The fake game: a Python child laying out the threat route in its own memory (tests/fake_game.py).
pub struct Fake {
    pub pid: i32,
    pub base: u64,
    pub obj: HashMap<String, u64>,
    /// The fake image's SizeOfImage (it depends on the fake's --build).
    pub size: u64,
    input: Option<File>,
    output: Receiver<String>,
    /// Started beside the binary: after it exits it stays a zombie until the binary exits.
    beside: bool,
}

fn parse_size(line: &str) -> u64 {
    match json::parse(line).ok().and_then(|v| v.get("size").cloned()) {
        Some(Value::Int(Some(n))) => n as u64,
        _ => panic!("fake game said {line:?}"),
    }
}

fn parse_info(line: &str) -> (u64, HashMap<String, u64>) {
    let v = json::parse(line).unwrap_or_else(|_| panic!("fake game said {line:?}"));
    let num = |v: &Value| match v {
        Value::Int(Some(n)) => *n as u64,
        _ => panic!("{line}"),
    };
    let base = num(v.get("base").unwrap());
    let Some(Value::Object(items)) = v.get("obj") else { panic!("{line}") };
    (base, items.iter().map(|(k, v)| (k.clone(), num(v))).collect())
}

impl Fake {
    /// Damage commands return metadata for the consumer's read/failure hooks.
    pub fn damage_frame(&mut self, frame: &str) -> Value {
        let f = self.input.as_mut().unwrap();
        writeln!(f, "damage {frame}").unwrap();
        f.flush().unwrap();
        let reply = json::parse(&self.recv()).expect("damage acknowledgement JSON");
        assert!(reply.get("error").is_none(), "damage frame rejected: {reply:?}");
        let metadata = reply.get("damage").expect("damage metadata").clone();
        if let Some(Value::Int(Some(base))) = metadata.get("image_base") {
            self.base = *base as u64;
        }
        metadata
    }

    fn recv(&self) -> String {
        self.output.recv_timeout(WAIT).expect("the fake game did not answer")
    }

    /// `f/i/p obj offset value` (see tests/fake_game.py).
    pub fn cmd(&mut self, line: &str) {
        assert!(!line.starts_with("damage "), "use damage_frame and apply its failure metadata");
        let f = self.input.as_mut().unwrap();
        writeln!(f, "{line}").unwrap();
        f.flush().unwrap();
        assert_eq!(self.recv().trim(), "ok", "{line}");
    }

    /// The fake execs a fresh fake game in place: same pid and (with --fixed) the same addresses.
    pub fn exec_image(&mut self, timestamp: u32) {
        let f = self.input.as_mut().unwrap();
        writeln!(f, "exec {timestamp}").unwrap();
        f.flush().unwrap();
        let (base, obj) = parse_info(&self.recv());
        assert_eq!((base, &obj), (self.base, &self.obj), "the new image moved");
    }

    /// Lets the fake exit. Started beside the binary, it stays a zombie child of the binary (which never waits
    /// for it) until the binary exits, so its pid cannot be reused meanwhile and this waits until it is one. A
    /// --launch child is reaped by the binary: wait for the binary instead (its pid is not looked at again).
    pub fn exit(&mut self) {
        let f = self.input.as_mut().unwrap();
        writeln!(f, "exit").unwrap();
        f.flush().unwrap();
        if !self.beside {
            return;
        }
        let deadline = Instant::now() + WAIT;
        while fs::read(format!("/proc/{}/cmdline", self.pid)).is_ok_and(|c| !c.is_empty()) {
            assert!(Instant::now() < deadline, "the fake game did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Reads the fake's memory directly (it is a descendant of this test).
    pub fn read_mem(&self, addr: u64, size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; size];
        let local = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: size };
        let remote = libc::iovec { iov_base: addr as usize as *mut libc::c_void, iov_len: size };
        // SAFETY: local describes buf; the remote range is only read.
        let n = unsafe { libc::process_vm_readv(self.pid, &local, 1, &remote, 1, 0) };
        assert_eq!(n, size as isize);
        buf
    }

    fn close(&mut self) {
        self.input.take(); // EOF: the fake game exits if it still runs
    }
}

/// How the fake game is started.
#[derive(Clone, Copy, PartialEq)]
pub enum Start {
    /// No fake game.
    None,
    /// Started by a shell that then execs into the binary: a real child of the binary, but not a --launch child.
    Beside,
    /// The binary's --launch child (the binary gets `--text --launch <the fake>`).
    Launch,
}

pub struct Opts {
    pub args: Vec<String>,
    pub start: Start,
    pub fake_args: Vec<String>,
    pub control: Vec<(String, String)>,
    pub env: Vec<(String, String)>,
    /// The program under test: DeHulud unless `combat()`.
    pub bin: PathBuf,
}

impl Opts {
    pub fn new(start: Start) -> Opts {
        Opts { args: vec!["--text".into()], start, fake_args: vec![], control: vec![], env: vec![], bin: bin() }
    }
    /// Runs DeHulud Combat instead of DeHulud.
    #[allow(dead_code)]
    pub fn combat(mut self) -> Opts {
        self.bin = combat_bin();
        self
    }
    pub fn fake(mut self, args: &[&str]) -> Opts {
        self.fake_args = args.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn args(mut self, args: &[&str]) -> Opts {
        self.args = args.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn set(mut self, k: &str, v: &str) -> Opts {
        self.control.push((k.into(), v.into()));
        self
    }
    pub fn env(mut self, k: &str, v: &str) -> Opts {
        self.env.push((k.into(), v.into()));
        self
    }
}

/// One run of the binary under test.
pub struct Run {
    pub dir: TempDir,
    child: Child,
    stdin: Option<ChildStdin>,
    ctl: PathBuf,
    control: Vec<(String, String)>,
    pub trace: PathBuf,
    pub fake: Option<Fake>,
    stdout: Arc<Mutex<Vec<String>>>,
    stderr: Arc<Mutex<Vec<String>>>,
    status: Option<i32>,
}

fn collect(r: impl Read + Send + 'static) -> Arc<Mutex<Vec<String>>> {
    let out = Arc::new(Mutex::new(Vec::new()));
    let o = out.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let Ok(line) = line else { break };
            o.lock().unwrap().push(line);
        }
    });
    out
}

impl Run {
    pub fn start(o: Opts) -> Run {
        let dir = TempDir::new("run");
        let ctl = dir.path().join("control");
        let trace = dir.path().join("trace.jsonl");
        for sub in ["cfg", "home"] {
            fs::create_dir(dir.path().join(sub)).unwrap();
        }
        let mut control = vec![
            ("poll_ms".to_string(), "20".to_string()),
            ("scan_ms".to_string(), "0".to_string()),
            ("trace".to_string(), trace.display().to_string()),
        ];
        control.extend(o.control.clone());
        write_control(&ctl, &control);
        let (fin, fout, pidf) = (dir.path().join("in"), dir.path().join("out"), dir.path().join("pid"));
        let fake_cmd =
            r#"exec "$DH_PY" -B "$DH_FAKE" DuneSandbox-Win64-Shipping.exe $DH_FAKE_ARGS <"$DH_IN" >"$DH_OUT""#;
        let mut cmd = match o.start {
            Start::None => {
                let mut c = Command::new(&o.bin);
                c.args(&o.args);
                c
            }
            Start::Beside => {
                let mut c = Command::new("/bin/sh");
                c.arg("-c")
                    .arg(format!(r#"( {fake_cmd} ) & echo $! >"$DH_PIDF"; exec "$DH_BIN" "$@""#))
                    .arg("sh")
                    .args(&o.args);
                c
            }
            Start::Launch => {
                let mut c = Command::new(&o.bin);
                c.args(&o.args).args(["--launch", "/bin/sh", "-c", &format!(r#"echo $$ >"$DH_PIDF"; {fake_cmd}"#)]);
                c
            }
        };
        if o.start != Start::None {
            mkfifo(&fin);
            mkfifo(&fout);
        }
        cmd.env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()))
            .env("LANG", "C.UTF-8")
            .env("HOME", dir.path().join("home"))
            .env("XDG_CONFIG_HOME", dir.path().join("cfg"))
            .env("TMPDIR", dir.path())
            .env("DEHULUD_TEST_SOURCE", &ctl)
            .env("DH_PY", python())
            .env("DH_FAKE", repo().join("tests").join("fake_game.py"))
            .env("DH_FAKE_ARGS", o.fake_args.join(" "))
            .env("DH_IN", &fin)
            .env("DH_OUT", &fout)
            .env("DH_PIDF", &pidf)
            .env("DH_BIN", &o.bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &o.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("start the binary");
        let stdin = child.stdin.take();
        let stdout = collect(child.stdout.take().unwrap());
        let stderr = collect(child.stderr.take().unwrap());
        let mut run = Run { dir, child, stdin, ctl, control, trace, fake: None, stdout, stderr, status: None };
        if o.start != Start::None {
            // O_RDWR never blocks on a FIFO (Linux); the fake game gets EOF on its stdin when this end closes.
            let input = OpenOptions::new().read(true).write(true).open(&fin).unwrap();
            let output = OpenOptions::new().read(true).write(true).open(&fout).unwrap();
            let output = line_channel(output);
            let first = output.recv_timeout(WAIT).expect("the fake game did not start");
            let (base, obj) = parse_info(&first);
            let size = parse_size(&first);
            let deadline = Instant::now() + WAIT;
            let pid = loop {
                if let Some(p) = fs::read_to_string(&pidf).ok().and_then(|s| s.trim().parse().ok()) {
                    break p;
                }
                assert!(Instant::now() < deadline, "no pid file");
                std::thread::sleep(Duration::from_millis(5));
            };
            run.fake =
                Some(Fake { pid, base, obj, size, input: Some(input), output, beside: o.start == Start::Beside });
        }
        run
    }

    /// Points the binary's test source at the fake game (pid and image base), plus extra keys.
    pub fn attach(&mut self, extra: &[(&str, &str)]) {
        let f = self.fake.as_ref().unwrap();
        let (pid, base, size) = (f.pid.to_string(), f.base.to_string(), f.size.to_string());
        let mut keys = vec![("pid", pid.as_str()), ("base", base.as_str()), ("size", size.as_str())];
        keys.extend_from_slice(extra);
        self.set(&keys);
    }

    pub fn set(&mut self, keys: &[(&str, &str)]) {
        for (k, v) in keys {
            self.control.push((k.to_string(), v.to_string()));
        }
        write_control(&self.ctl, &self.control);
    }

    /// One synchronous frame; replace failure ranges instead of accumulating them.
    pub fn damage_step(&mut self, step: i64, at_ms: i64, enabled: &[&str], failures: &Value) -> Value {
        self.control.retain(|(key, _)| {
            !["fail_range", "damage_step", "at_ms", "enabled", "base", "pid", "size"].contains(&key.as_str())
        });
        let fake = self.fake.as_ref().unwrap();
        self.control.extend([
            ("damage_step".into(), step.to_string()),
            ("at_ms".into(), at_ms.to_string()),
            ("enabled".into(), enabled.join(",")),
            ("base".into(), fake.base.to_string()),
            ("pid".into(), fake.pid.to_string()),
            ("size".into(), fake.size.to_string()),
        ]);
        let Value::Array(ranges) = failures else { panic!("failure ranges must be an array") };
        for range in ranges {
            let Value::Array(pair) = range else { panic!("failure range must be a pair") };
            let [Value::Int(Some(start)), Value::Int(Some(end))] = pair.as_slice() else { panic!("invalid range") };
            self.control.push(("fail_range".into(), format!("{start}:{end}")));
        }
        write_control(&self.ctl, &self.control);
        self.wait_for("damage frame", |poll| poll.get("damage_step") == Some(&Value::Int(Some(step))))
    }

    pub fn fake(&mut self) -> &mut Fake {
        self.fake.as_mut().unwrap()
    }

    pub fn cmd(&mut self, line: &str) {
        self.fake().cmd(line);
    }

    /// Every trace line so far.
    pub fn traced(&self) -> Vec<Value> {
        fs::read_to_string(&self.trace).unwrap_or_default().lines().filter_map(|l| json::parse(l).ok()).collect()
    }

    pub fn polls(&self) -> Vec<Value> {
        self.traced().into_iter().filter(|v| v.get("event") == Some(&Value::Str("poll".into()))).collect()
    }

    fn last_n(&self) -> i64 {
        self.polls().last().map_or(0, |p| int(p, "n"))
    }

    /// The first poll that satisfies `pred`.
    pub fn wait_for(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(p) = self.polls().into_iter().find(|p| pred(p)) {
                return p;
            }
            assert!(
                Instant::now() < deadline,
                "no poll with {what}; polls: {:?}\nstderr: {:?}",
                self.polls().last(),
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The first poll that started after this call (one complete poll of the current state).
    pub fn poll(&self) -> Value {
        let after = self.last_n() + 1;
        self.wait_for(&format!("n > {after}"), |p| int(p, "n") > after)
    }

    /// The poll that attached the game, once the source points at it.
    pub fn first_attached(&self) -> Value {
        self.wait_for("an attached game", |p| p.get("build") != Some(&Value::Null))
    }

    pub fn stdout(&self) -> Vec<String> {
        self.stdout.lock().unwrap().clone()
    }

    pub fn stderr(&self) -> Vec<String> {
        self.stderr.lock().unwrap().clone()
    }

    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    /// Waits for the binary to exit on its own; its exit status.
    pub fn wait(&mut self) -> i32 {
        if let Some(s) = self.status {
            return s;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                use std::os::unix::process::ExitStatusExt;
                let code = st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0));
                self.status = Some(code);
                std::thread::sleep(Duration::from_millis(50)); // let the output threads finish
                return code;
            }
            assert!(Instant::now() < deadline, "the binary did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Asks the binary to stop (control file) and waits for it.
    pub fn stop(&mut self) -> i32 {
        self.set(&[("stop", "1")]);
        self.close_stdin();
        self.wait()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.status.is_none() {
            self.set(&[("stop", "1")]);
            self.stdin.take();
            if let Some(f) = self.fake.as_mut() {
                f.close(); // a --launch DeHulud waits for its game: end the game too
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill(); // our own child, stuck
                let _ = self.child.wait();
            }
        }
        if let Some(f) = self.fake.as_mut() {
            f.close();
        }
    }
}

fn write_control(path: &Path, keys: &[(String, String)]) {
    let tmp = path.with_extension("tmp");
    let text: String = keys.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    fs::write(&tmp, text).unwrap();
    fs::rename(&tmp, path).unwrap();
}

pub fn int(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Int(Some(n))) => *n,
        other => panic!("{k}: {other:?} in {v:?}"),
    }
}

pub fn text(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::Str(s)) => s.clone(),
        other => panic!("{k}: {other:?} in {v:?}"),
    }
}

/// (status, percent) of a poll.
pub fn sp(v: &Value) -> (String, Option<i64>) {
    let pct = match v.get("percent") {
        Some(Value::Int(Some(n))) => Some(*n),
        _ => None,
    };
    (text(v, "status"), pct)
}

pub fn fraction(v: &Value) -> Option<f64> {
    match v.get("fraction") {
        Some(Value::Float(f)) => Some(*f),
        Some(Value::Int(Some(n))) => Some(*n as f64),
        _ => None,
    }
}

pub fn is_null(v: &Value, k: &str) -> bool {
    v.get(k) == Some(&Value::Null)
}

/// The kernel's ptrace_scope: the tests read the fake game as its ancestor, which 0 and 1 allow.
pub fn readable() -> bool {
    match fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok().and_then(|s| s.trim().parse::<i32>().ok()) {
        Some(s) if s >= 2 => {
            eprintln!("skipped: ptrace_scope {s} refuses reading even a child process");
            false
        }
        _ => true,
    }
}

/// Writes `data` to `path` with mode 0700 (a small script).
pub fn script(path: &Path, data: &str) {
    let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o700).open(path).unwrap();
    f.write_all(data.as_bytes()).unwrap();
}
