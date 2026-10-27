//! The binary exactly as shipped: built WITHOUT the test feature, so with its real /proc scan, the maps parser
//! and process_vm_readv. It runs against the fake game inside a private PID namespace (`unshare --user
//! --map-root-user --pid --fork --mount-proc`, no privileges needed), whose /proc lists only the processes this
//! test starts there, so the scan never sees another process of this computer. The fake game's image is mapped
//! from a file named DuneSandbox-Win64-Shipping.exe (tests/fake_file_image.py), as Wine maps the real game.
//!
//! `cargo test --release --test namespace`; DEHULUD_BIN=<path> tests another build (the musl release binary).
#![cfg(not(feature = "test-source"))]

mod common;

use common::*;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

#[test]
fn unmodified_binary_reads_the_fake_game_in_a_private_pid_namespace() {
    let ok = Command::new("unshare")
        .args(["--user", "--map-root-user", "--pid", "--fork", "--mount-proc", "true"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("skipped: no unprivileged PID namespace here (unshare)");
        return;
    }
    let bin = std::env::var_os("DEHULUD_BIN").map_or_else(bin, std::path::PathBuf::from);
    let tmp = TempDir::new("ns");
    let (fin, fout) = (tmp.path().join("in"), tmp.path().join("out"));
    for f in [&fin, &fout] {
        let c = std::ffi::CString::new(f.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }
    std::fs::create_dir(tmp.path().join("cfg")).unwrap();
    // PID 1 of the namespace is this shell: the fake game and DeHulud run beside it until it reads a line.
    let script =
        r#""$DH_PY" -B "$DH_WRAP" "$DH_EXE" "$DH_DIR" <"$DH_IN" >"$DH_OUT" & "$DH_BIN" --text & read line; exit 0"#;
    let mut ns = Command::new("unshare")
        .args(["--user", "--map-root-user", "--pid", "--fork", "--mount-proc", "/bin/sh", "-c", script])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env("DH_PY", python())
        .env("DH_WRAP", repo().join("rust").join("tests").join("fake_file_image.py"))
        .env("DH_EXE", EXE)
        .env("DH_DIR", tmp.path())
        .env("DH_IN", &fin)
        .env("DH_OUT", &fout)
        .env("DH_BIN", &bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = OpenOptions::new().read(true).write(true).open(&fin).unwrap();
    let output = OpenOptions::new().read(true).write(true).open(&fout).unwrap();
    let mut fake_out = BufReader::new(output).lines();
    let info = fake_out.next().unwrap().unwrap();
    assert!(info.starts_with("{\"base\""), "{info}");
    let (tx, rx) = channel();
    let stdout = ns.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut seen = Vec::new();
    let mut expect = |want: &str| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Ok(line) = rx.recv_timeout(Duration::from_millis(100)) {
                seen.push(line.clone());
                if line == want {
                    return;
                }
            }
        }
        panic!("no line {want:?}; got {seen:?}");
    };
    let mut cmd = |line: &str| {
        writeln!(input, "{line}").unwrap();
        input.flush().unwrap();
        assert_eq!(fake_out.next().unwrap().unwrap(), "ok");
    };
    expect("DeHulud  42% [########............] on foot");
    cmd("p pc 0x388 veh");
    expect("DeHulud  80% [################....] vehicle");
    cmd("f veh_comp 0x348 0");
    expect("DeHulud   -  [....................] vehicle, no threshold");
    writeln!(input, "exit").unwrap(); // the game exits
    input.flush().unwrap();
    expect("DeHulud   -  [....................] waiting for game");
    ns.stdin.take().unwrap().write_all(b"\n").unwrap(); // PID 1 exits: the namespace ends
    assert!(ns.wait().unwrap().success());
}
