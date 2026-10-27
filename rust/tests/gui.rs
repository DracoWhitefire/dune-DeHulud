//! GUI smoke test: the real window of the test build, but ONLY on an owned, private display.
//!
//! Like tests/test_gui.py of the Python version, it never uses $DISPLAY / $WAYLAND_DISPLAY of the session
//! running the tests: it starts its own headless compositor (`kwin_wayland --virtual --xwayland`) under a private
//! D-Bus session with private HOME and XDG folders, and runs the probe below inside it (this same test program,
//! started again by the compositor). The probe refuses any X display that existed before the compositor started,
//! drives the window with synthetic X events (to DeHulud's own windows only), reads its state back from the X
//! server and its pixels, and writes a JSON result that this test checks. Skipped when kwin_wayland, Xwayland or
//! dbus-run-session is missing.
#![cfg(feature = "test-source")]

mod common;

use common::*;
use dehulud::display::{self, LocalDisplay};
use dehulud::json::{self, Value, quote};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self, AtomEnum, ButtonPressEvent, ConnectionExt as _, EventMask, ImageFormat, KeyButMask, MotionNotifyEvent,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};

fn which(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

#[test]
fn window_on_owned_display() {
    let missing: Vec<&str> =
        ["kwin_wayland", "Xwayland", "dbus-run-session"].into_iter().filter(|t| !which(t)).collect();
    if !missing.is_empty() {
        eprintln!("skipped: no owned display server available (missing: {})", missing.join(", "));
        return;
    }
    let tmp = TempDir::new("gui");
    let dirs: Vec<(&str, PathBuf)> =
        ["home", "run", "cfg", "data", "cache", "state"].iter().map(|k| (*k, tmp.path().join(k))).collect();
    for (_, d) in &dirs {
        fs::create_dir(d).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let dir = |k: &str| dirs.iter().find(|d| d.0 == k).unwrap().1.clone();
    let mut before: Vec<String> = fs::read_dir("/tmp/.X11-unix")
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    before.sort();
    let result = tmp.path().join("result.json");
    let app = tmp.path().join("app.sh");
    let me = std::env::current_exe().unwrap();
    script(
        &app,
        &format!(
            "#!/bin/sh\nexec {} --ignored --exact probe_inside_owned_compositor --nocapture --test-threads=1 >{} 2>&1\n",
            dehulud::help::shell_quote(&me.to_string_lossy()),
            dehulud::help::shell_quote(&tmp.path().join("probe.log").to_string_lossy())
        ),
    );
    let log = fs::File::create(tmp.path().join("kwin.log")).unwrap();
    let mut kwin: Child = Command::new("dbus-run-session")
        .args([
            "--",
            "kwin_wayland",
            "--virtual",
            "--xwayland",
            "--no-lockscreen",
            "--no-global-shortcuts",
            "--no-kactivities",
        ])
        .args(["--socket", "dehulud-rs-test", "--width", "1280", "--height", "800", "--exit-with-session"])
        .arg(&app)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()))
        .env("LANG", "C.UTF-8")
        .env("HOME", dir("home"))
        .env("XDG_RUNTIME_DIR", dir("run"))
        .env("XDG_CONFIG_HOME", dir("cfg"))
        .env("XDG_DATA_HOME", dir("data"))
        .env("XDG_CACHE_HOME", dir("cache"))
        .env("XDG_STATE_HOME", dir("state"))
        .env("TMPDIR", tmp.path())
        .env("DEHULUD_GUI_RESULT", &result)
        .env("DEHULUD_GUI_BEFORE", if before.is_empty() { "-".to_string() } else { before.join(",") })
        .env("DEHULUD_GUI_BIN", bin())
        .env("DEHULUD_GUI_DIR", tmp.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("dbus-run-session");
    let deadline = Instant::now() + Duration::from_secs(120);
    while kwin.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = kwin.kill(); // our own child, stuck
            let _ = kwin.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let tail = |name: &str| {
        let t = fs::read_to_string(tmp.path().join(name)).unwrap_or_default();
        t.chars().rev().take(3000).collect::<Vec<_>>().into_iter().rev().collect::<String>()
    };
    let Ok(text) = fs::read_to_string(&result) else {
        panic!("probe produced no result\n{}\n{}", tail("probe.log"), tail("kwin.log"));
    };
    if let Some(keep) = std::env::var_os("DEHULUD_GUI_KEEP") {
        // a developer's copy of the result and the window images
        let keep = PathBuf::from(keep);
        let _ = fs::create_dir_all(&keep);
        for e in fs::read_dir(tmp.path()).unwrap().flatten() {
            if e.path().is_file() {
                let _ = fs::copy(e.path(), keep.join(e.file_name()));
            }
        }
    }
    let r = json::parse(&text).unwrap();
    let get = |k: &str| r.get(k).unwrap_or_else(|| panic!("{k} missing in {text}\n{}", tail("probe.log"))).clone();
    let s = |v: &Value| match v {
        Value::Str(s) => s.clone(),
        o => panic!("{o:?}"),
    };
    let b = |v: Value| v == Value::Bool(true);
    let ints = |v: Value| match v {
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::Int(Some(n)) => *n,
                o => panic!("{o:?}"),
            })
            .collect::<Vec<i64>>(),
        o => panic!("{o:?}"),
    };
    assert!(r.get("refused").is_none(), "{text}");
    let failures = match get("failures") {
        Value::Array(a) => a.iter().map(s).collect::<Vec<_>>(),
        o => panic!("{o:?}"),
    };
    assert_eq!(failures, Vec::<String>::new(), "{}", tail("probe.log"));
    assert!(s(&get("socket")).starts_with("/tmp/.X11-unix/X") || s(&get("socket")).starts_with("@/tmp/.X11-unix/X"));
    assert!(b(get("unix_sockets_only")));
    assert_eq!(s(&get("title")), "DeHulud");
    assert!(b(get("above")), "_NET_WM_STATE_ABOVE");
    assert!(b(get("frameless")), "override-redirect");
    assert_eq!(ints(get("size")), [180, 44]);
    assert_eq!(ints(get("position")), [1280 - 180 - 24, 48], "top-right of the primary monitor");
    let shown = match get("shown") {
        Value::Array(a) => a
            .iter()
            .map(|v| match v {
                Value::Array(x) => x.iter().map(s).collect::<Vec<_>>(),
                o => panic!("{o:?}"),
            })
            .collect::<Vec<_>>(),
        o => panic!("{o:?}"),
    };
    assert_eq!(shown[0], ["42%", "on foot", "#c7df7a", ""]);
    assert_eq!(shown[1], ["-", "Unsupported game version", "#85969d", "PE 1700000000"]);
    assert_eq!(shown[2], ["100%", "vehicle", "#ee7868", ""]);
    assert_eq!(shown[3], ["-", "no permission", "#85969d", "right-click"]);
    assert!(b(get("lone_release_ignored")), "a release without a press chose a menu entry");
    assert_eq!(s(&get("help")), "FIX: sudo sysctl");
    assert!(b(get("help_window_mapped")));
    assert_eq!(s(&get("after_drag")), "{\"x\": 130, \"y\": 70, \"frame\": false}");
    assert_eq!(ints(get("dragged_to")), [130, 70]);
    assert!(s(&get("after_frame")).ends_with("\"frame\": true}"));
    assert!(b(get("framed_is_managed")), "a window manager frame around the framed window");
    assert_eq!(s(&get("after_reset")), "{\"x\": null, \"y\": null, \"frame\": true}");
    assert!(b(get("quit_exit_0")));
    // the framed window's position is saved as its frame's top-left: reopening puts it at the same place
    assert_eq!(s(&get("after_quit")), "{\"x\": 1076, \"y\": 48, \"frame\": true}");
}

// ---- the probe: runs inside the private compositor ------------------------------------------------------

struct Probe {
    conn: RustConnection<DefaultStream>,
    root: u32,
    trace: PathBuf,
    ctl: PathBuf,
    control: Vec<String>,
    failures: Vec<String>,
    out: Vec<(String, String)>,
    dir: PathBuf,
}

impl Probe {
    fn check(&mut self, ok: bool, what: String) {
        if !ok {
            eprintln!("FAILED: {what}");
            self.failures.push(what);
        }
    }
    fn put(&mut self, k: &str, json_value: String) {
        self.out.push((k.into(), json_value));
    }
    fn set(&mut self, line: &str) {
        self.control.push(line.into());
        let tmp = self.ctl.with_extension("tmp");
        fs::write(&tmp, self.control.join("\n") + "\n").unwrap();
        fs::rename(tmp, &self.ctl).unwrap();
    }
    fn events(&self) -> Vec<Value> {
        fs::read_to_string(&self.trace).unwrap_or_default().lines().filter_map(|l| json::parse(l).ok()).collect()
    }
    /// The first traced window event after `skip` events that matches.
    fn wait(&self, what: &str, skip: usize, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = self.events().into_iter().skip(skip).find(|v| v.get("window").is_some() && pred(v)) {
                return v;
            }
            assert!(Instant::now() < deadline, "no window event with {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn n(&self) -> usize {
        self.events().len()
    }
    fn send(&self, win: u32, kind: &str, button: u8, root: (i16, i16), at: (i16, i16)) {
        let (response_type, mask) = match kind {
            "press" => (xproto::BUTTON_PRESS_EVENT, EventMask::BUTTON_PRESS),
            "release" => (xproto::BUTTON_RELEASE_EVENT, EventMask::BUTTON_RELEASE),
            _ => unreachable!(),
        };
        let ev = ButtonPressEvent {
            response_type,
            detail: button,
            sequence: 0,
            time: 0,
            root: self.root,
            event: win,
            child: x11rb::NONE,
            root_x: root.0,
            root_y: root.1,
            event_x: at.0,
            event_y: at.1,
            state: KeyButMask::default(),
            same_screen: true,
        };
        self.conn.send_event(false, win, mask, ev).unwrap();
        self.conn.flush().unwrap();
    }
    fn motion(&self, win: u32, root: (i16, i16), at: (i16, i16)) {
        let ev = MotionNotifyEvent {
            response_type: xproto::MOTION_NOTIFY_EVENT,
            detail: xproto::Motion::NORMAL,
            sequence: 0,
            time: 0,
            root: self.root,
            event: win,
            child: x11rb::NONE,
            root_x: root.0,
            root_y: root.1,
            event_x: at.0,
            event_y: at.1,
            state: KeyButMask::BUTTON1,
            same_screen: true,
        };
        self.conn.send_event(false, win, EventMask::POINTER_MOTION, ev).unwrap();
        self.conn.flush().unwrap();
    }
    fn title(&self, win: u32) -> String {
        let r =
            self.conn.get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 64).unwrap().reply().unwrap();
        String::from_utf8_lossy(&r.value).into_owned()
    }
    fn atom(&self, name: &str) -> u32 {
        self.conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom
    }
    fn above(&self, win: u32) -> bool {
        let state = self.atom("_NET_WM_STATE");
        let above = self.atom("_NET_WM_STATE_ABOVE");
        let r = self.conn.get_property(false, win, state, AtomEnum::ATOM, 0, 32).unwrap().reply().unwrap();
        r.value32().is_some_and(|mut v| v.any(|a| a == above))
    }
    fn root_pos(&self, win: u32) -> (i64, i64) {
        let t = self.conn.translate_coordinates(win, self.root, 0, 0).unwrap().reply().unwrap();
        (i64::from(t.dst_x), i64::from(t.dst_y))
    }
    fn managed(&self, win: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let t = self.conn.query_tree(win).unwrap().reply().unwrap();
            if t.parent != t.root {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// Opens the menu with a right-click on `win` and activates the entry `label`.
    fn menu(&self, win: u32, label: &str) {
        let (menu, at, root) = self.open_menu(win, label);
        self.send(menu, "press", 1, root, at);
        self.send(menu, "release", 1, root, at);
    }
    /// Opens the menu with a right-click on `win`: the menu window, and the point of entry `label` in it and on
    /// the root window.
    fn open_menu(&self, win: u32, label: &str) -> (u32, (i16, i16), (i16, i16)) {
        let (x, y) = self.root_pos(win);
        let n = self.n();
        let root = ((x + 60) as i16, (y + 30) as i16);
        self.send(win, "press", 3, root, (60, 30));
        self.send(win, "release", 3, root, (60, 30));
        let ev = self.wait("an open menu", n, |v| v.get("menu").is_some_and(|m| *m != Value::Null));
        let Some(Value::Int(Some(menu))) = ev.get("menu") else { unreachable!() };
        let menu = *menu as u32;
        let Some(Value::Array(items)) = ev.get("items") else { unreachable!() };
        let row = items
            .iter()
            .find_map(|it| match it {
                Value::Array(a) if a[0] == Value::Str(label.into()) => Some(a.clone()),
                _ => None,
            })
            .expect("menu entry");
        let (Value::Int(Some(y0)), Value::Int(Some(y1))) = (&row[1], &row[2]) else { unreachable!() };
        let at = (20i16, ((y0 + y1) / 2) as i16);
        let (mx, my) = self.root_pos(menu);
        std::thread::sleep(Duration::from_millis(100));
        self.keep(menu, &format!("menu-{}.ppm", label.split(' ').next().unwrap()));
        (menu, at, ((mx + 20) as i16, (my + i64::from(at.1)) as i16))
    }
    fn config(&self) -> String {
        fs::read_to_string(self.dir.join("cfg").join("dehulud").join("config.json")).unwrap_or_default()
    }
    /// The window's pixels as 0xRRGGBB (32 bits per pixel, the usual 24-bit TrueColor layout).
    fn pixels(&self, win: u32) -> Vec<u32> {
        self.image(win).2
    }
    fn image(&self, win: u32) -> (u16, u16, Vec<u32>) {
        let g = self.conn.get_geometry(win).unwrap().reply().unwrap();
        let img =
            self.conn.get_image(ImageFormat::Z_PIXMAP, win, 0, 0, g.width, g.height, !0).unwrap().reply().unwrap();
        assert_eq!(img.data.len(), usize::from(g.width) * usize::from(g.height) * 4, "32 bits per pixel");
        (
            g.width,
            g.height,
            img.data.chunks(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]) & 0xffffff).collect(),
        )
    }
    /// Keeps a picture of one of DeHulud's windows (for a developer; see DEHULUD_GUI_KEEP).
    fn keep(&self, win: u32, name: &str) {
        let (w, h, px) = self.image(win);
        save_ppm_sized(&self.dir.join(name), w, h, &px);
    }
}

fn save_ppm(path: &Path, px: &[u32]) {
    save_ppm_sized(path, 180, 44, px);
}

fn save_ppm_sized(path: &Path, w: u16, h: u16, px: &[u32]) {
    let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in px {
        data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    fs::write(path, data).unwrap();
}

/// (count, min x, max x, min y, max y) of the pixels of one colour.
fn ink(px: &[u32], rgb: u32) -> (usize, i64, i64, i64, i64) {
    let mut r = (0, i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for (i, p) in px.iter().enumerate() {
        if *p == rgb {
            let (x, y) = ((i % 180) as i64, (i / 180) as i64);
            r = (r.0 + 1, r.1.min(x), r.2.max(x), r.3.min(y), r.4.max(y));
        }
    }
    r
}

/// The socket inodes of a process (our own child) and whether each is a Unix socket and none an IP socket.
fn unix_sockets_only(pid: u32) -> (bool, usize) {
    let mut inodes = Vec::new();
    for e in fs::read_dir(format!("/proc/{pid}/fd")).unwrap().flatten() {
        if let Ok(target) = fs::read_link(e.path()) {
            let t = target.to_string_lossy().into_owned();
            if let Some(ino) = t.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
                inodes.push(ino.to_string());
            }
        }
    }
    let table = |name: &str| fs::read_to_string(format!("/proc/{pid}/net/{name}")).unwrap_or_default();
    let has = |text: &str, ino: &str| text.lines().skip(1).any(|l| l.split_whitespace().any(|w| w == ino));
    let unix = table("unix");
    let ip: String = ["tcp", "tcp6", "udp", "udp6", "raw", "raw6"].iter().map(|n| table(n)).collect();
    let ok = !inodes.is_empty() && inodes.iter().all(|i| has(&unix, i) && !has(&ip, i));
    (ok, inodes.len())
}

#[test]
#[ignore = "runs only inside the private compositor started by window_on_owned_display"]
fn probe_inside_owned_compositor() {
    let result = PathBuf::from(std::env::var_os("DEHULUD_GUI_RESULT").expect("started by window_on_owned_display"));
    let before = std::env::var("DEHULUD_GUI_BEFORE").unwrap_or_default();
    let display_name = std::env::var("DISPLAY").unwrap_or_default();
    let number =
        display_name.strip_prefix(':').map(|r| r.split('.').next().unwrap_or("").to_string()).unwrap_or_default();
    if number.is_empty() || before.split(',').any(|x| x == format!("X{number}")) {
        // never the session's own display
        fs::write(&result, format!("{{\"refused\": {}}}", quote(&display_name))).unwrap();
        return;
    }
    let dir = PathBuf::from(std::env::var_os("DEHULUD_GUI_DIR").unwrap());
    let bin = PathBuf::from(std::env::var_os("DEHULUD_GUI_BIN").unwrap());
    let d = LocalDisplay::parse(&display_name).expect("a local display");
    let c = display::connect(&d).expect("the private display");
    let root = c.conn.setup().roots[c.screen].root;
    let ctl = dir.join("control");
    let trace = dir.join("trace.jsonl");
    let mut p = Probe {
        conn: c.conn,
        root,
        trace: trace.clone(),
        ctl: ctl.clone(),
        control: vec![],
        failures: vec![],
        out: vec![],
        dir: dir.clone(),
    };
    for line in [
        "poll_ms=40".to_string(),
        format!("trace={}", trace.display()),
        "reading=on foot|42|0.425||".into(),
        "reading=Unsupported game version|-|-|PE 1700000000|PE timestamp 1700000000".into(),
        "reading=vehicle|100|1.0||".into(),
        "reading=no permission|-|-|right-click|FIX: sudo sysctl".into(),
        "step=0".into(),
    ] {
        p.set(&line);
    }
    let mut child = Command::new(&bin)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("cfg"))
        .env("DISPLAY", &display_name)
        .env("XAUTHORITY", std::env::var("XAUTHORITY").unwrap_or_default())
        .env("DEHULUD_TEST_SOURCE", &ctl)
        .stdout(fs::File::create(dir.join("dehulud.out")).unwrap())
        .stderr(fs::File::create(dir.join("dehulud.err")).unwrap())
        .spawn()
        .unwrap();
    let first = p.wait("the window", 0, |v| matches!(v.get("window"), Some(Value::Int(Some(w))) if *w != 0));
    let Some(Value::Int(Some(win))) = first.get("window") else { unreachable!() };
    let win = *win as u32;
    let socket = match first.get("socket") {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    };
    p.put("socket", quote(&socket));
    let (only_unix, n) = unix_sockets_only(child.id());
    p.put("unix_sockets_only", only_unix.to_string());
    eprintln!("sockets of DeHulud: {n}, all Unix: {only_unix}");

    // the window as the X server sees it
    p.put("title", quote(&p.title(win)));
    p.put("above", p.above(win).to_string());
    let attrs = p.conn.get_window_attributes(win).unwrap().reply().unwrap();
    p.put("frameless", attrs.override_redirect.to_string());
    let g = p.conn.get_geometry(win).unwrap().reply().unwrap();
    p.put("size", format!("[{}, {}]", g.width, g.height));
    let pos = p.root_pos(win);
    p.put("position", format!("[{}, {}]", pos.0, pos.1));

    // four readings: what is shown, and where its pixels are
    let mut shown = Vec::new();
    let statuses = ["on foot", "Unsupported game version", "vehicle", "no permission"];
    for (step, status) in statuses.iter().enumerate() {
        let n = p.n();
        p.set(&format!("step={step}"));
        let ev = p.wait(
            status,
            n,
            |v| matches!(v.get("shown"), Some(Value::Array(a)) if a[1] == Value::Str((*status).into())),
        );
        p.wait("one more tick", p.n(), |_| true);
        let Some(Value::Array(sh)) = ev.get("shown") else { unreachable!() };
        let sh: Vec<String> = sh
            .iter()
            .map(|v| match v {
                Value::Str(s) => quote(s),
                _ => "null".into(),
            })
            .collect();
        shown.push(format!("[{}]", sh.join(", ")));
        let px = p.pixels(win);
        save_ppm(&dir.join(format!("window-{step}.ppm")), &px);
        let (pct, hint, status_ink, label) =
            (ink(&px, 0xf0f0f0), ink(&px, 0xb8c0c4), ink(&px, 0xaab4b8), ink(&px, 0xd8cfb4));
        p.check(
            pct.0 > 0 && pct.1 >= 131 && pct.2 < 180,
            format!("step {step}: the percent never covers the bar: {pct:?}"),
        );
        p.check(
            status_ink.0 > 0 && status_ink.3 >= 20 && status_ink.4 < 44,
            format!("step {step}: status line {status_ink:?}"),
        );
        p.check(label.0 > 0 && label.2 < 44, format!("step {step}: WORM {label:?}"));
        let want_hint = step == 1 || step == 3;
        p.check((hint.0 > 0) == want_hint, format!("step {step}: hint shown {hint:?}"));
        if want_hint {
            p.check(hint.1 > 44 && hint.2 < 130, format!("step {step}: hint inside the bar {hint:?}"));
        }
        let bar = [0xc7df7a, 0x2a2c2e, 0xee7868, 0x2a2c2e][step];
        p.check(px[7 * 180 + 45] == bar, format!("step {step}: bar pixel {:06x}", px[7 * 180 + 45]));
        if step == 0 {
            p.check(
                px[7 * 180 + 79] == 0xc7df7a && px[7 * 180 + 81] != 0xc7df7a,
                "42 % fills 36 of 86 bar pixels".into(),
            );
        }
        if step == 2 {
            p.check(px[17 * 180 + 129] == 0xee7868, "100 % fills the whole bar".into());
        }
    }
    p.put("shown", format!("[{}]", shown.join(", ")));

    // a button release alone (as of the right-click that opened the menu) never chooses an entry, even Quit
    let (menu, at, root_at) = p.open_menu(win, "Quit");
    p.send(menu, "release", 3, root_at, at);
    std::thread::sleep(Duration::from_millis(300));
    let alive = child.try_wait().unwrap().is_none();
    p.put("lone_release_ignored", alive.to_string());
    let n = p.n();
    let (x0, y0) = p.root_pos(win);
    p.send(win, "press", 1, ((x0 + 60) as i16, (y0 + 30) as i16), (60, 30)); // a click elsewhere closes the menu
    p.send(win, "release", 1, ((x0 + 60) as i16, (y0 + 30) as i16), (60, 30));
    p.wait("the menu closed", n, |v| v.get("event") == Some(&Value::Str("menu closed".into())));

    // Help / details (the current reading's detail)
    let n = p.n();
    p.menu(win, "Help / details");
    let ev = p.wait("a help window", n, |v| matches!(v.get("help"), Some(Value::Int(Some(_)))));
    let help_text = match ev.get("help_text") {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    };
    p.put("help", quote(&help_text));
    let Some(Value::Int(Some(help_win))) = ev.get("help") else { unreachable!() };
    let help_win = *help_win as u32;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mapped = loop {
        // a managed window is shown once the window manager has mapped it
        let viewable =
            p.conn.get_window_attributes(help_win).unwrap().reply().unwrap().map_state == xproto::MapState::VIEWABLE;
        if viewable || Instant::now() > deadline {
            break viewable;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    p.check(p.title(help_win) == "DeHulud help", "help window title".into());
    p.put("help_window_mapped", mapped.to_string());
    std::thread::sleep(Duration::from_millis(200));
    p.keep(help_win, "help.ppm");
    let n = p.n();
    p.send(help_win, "press", 1, (0, 0), (10, 10)); // a click closes it
    p.wait("the help window closed", n, |v| v.get("help") == Some(&Value::Null));

    // drag with the left button
    let n = p.n();
    let (x0, y0) = p.root_pos(win);
    p.send(win, "press", 1, ((x0 + 4) as i16, (y0 + 2) as i16), (4, 2));
    p.motion(win, (100, 100), (4, 2));
    p.motion(win, (134, 72), (4, 2));
    p.send(win, "release", 1, (134, 72), (4, 2));
    p.wait("the move", n, |v| v.get("event") == Some(&Value::Str("moved".into())));
    p.put("after_drag", quote(&p.config()));
    let moved = p.root_pos(win);
    p.put("dragged_to", format!("[{}, {}]", moved.0, moved.1));

    // window frame on: a managed window with a title bar
    let n = p.n();
    p.menu(win, "Window frame");
    let ev = p.wait("the framed window", n, |v| v.get("frame") == Some(&Value::Bool(true)));
    let Some(Value::Int(Some(framed))) = ev.get("window") else { unreachable!() };
    let framed = *framed as u32;
    p.put("after_frame", quote(&p.config()));
    let attrs = p.conn.get_window_attributes(framed).unwrap().reply().unwrap();
    p.check(!attrs.override_redirect, "the framed window is not override-redirect".into());
    p.put("framed_is_managed", p.managed(framed).to_string());
    p.check(p.title(framed) == "DeHulud", "framed window title".into());
    p.check(p.above(framed), "framed window keeps _NET_WM_STATE_ABOVE".into());

    // reset position
    let n = p.n();
    p.menu(framed, "Reset position");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !p.config().contains("null") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = n;
    p.put("after_reset", quote(&p.config()));

    // quit
    p.menu(framed, "Quit");
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st.code();
        }
        if Instant::now() > deadline {
            let _ = child.kill(); // our own child, stuck
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    p.put("quit_exit_0", (code == Some(0)).to_string());
    p.put("after_quit", quote(&p.config()));

    let failures: Vec<String> = p.failures.iter().map(|f| quote(f)).collect();
    p.out.push(("failures".into(), format!("[{}]", failures.join(", "))));
    let body: Vec<String> = p.out.iter().map(|(k, v)| format!("{}: {v}", quote(k))).collect();
    fs::write(&result, format!("{{{}}}", body.join(", "))).unwrap();
}
