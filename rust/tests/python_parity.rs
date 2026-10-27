//! Differential test against the reference implementation: the repository's `dehulud.py` (imported, never
//! run as a meter) and this crate are given the same inputs, and every printed text and every rule must come
//! out the same: help texts, the unsupported-build message, the terminal line, the value rules, display names,
//! placement, quoting, config parsing, PE identity and the maps parser. Needs a Python 3 (see common::python).

mod common;

use common::*;
use dehulud::config::{self, Monitor};
use dehulud::display::LocalDisplay;
use dehulud::help;
use dehulud::json::{self, Value, quote};
use dehulud::meter::{self, Reading};
use dehulud::text::format_line;
use dehulud::window::{bar_color, hex};
use std::process::{Command, Stdio};

const PROG: &str = "/opt/My Tools/Bob's \"meters\"/dehulud";

const SCRIPT: &str = r#"
import json, os, shlex, sys
sys.path.insert(0, sys.argv[1])
import dehulud
dehulud.__file__ = sys.argv[2]
inp = json.load(sys.stdin)
out = {}
scopes = [0, 1, 2, 3, None, 7]
out["help"] = [dehulud.permission_help(s, l) for s in scopes for l in (False, True)]
out["outside"] = [dehulud.outside_launch_help(4242, s) for s in (1, 2, 3, 7)]
out["note"] = [dehulud.OUTSIDE % (9, "0"), dehulud.OUTSIDE % (9, "not readable")]
known = ", ".join("build %s (PE timestamp %d)" % (b["build"], k[0]) for k, b in dehulud.BUILDS.items())
import time
out["unsupported"] = [dehulud.UNSUPPORTED % (t, time.strftime("%Y-%m-%d %H:%M", time.gmtime(t)), s, known)
                      for t, s in inp["idents"]]
out["lines"] = [dehulud.format_line(dehulud.Reading(st, p, f, d, h)) for st, p, f, d, h in inp["readings"]]
out["values"] = [list(dehulud.reading_for(m, b, t))[:3] for m, b, t in inp["values"]]
def disp(name):
    r = dehulud.local_display(name)
    if r is None:
        return None
    n, _, s = r[len("unix/:"):].partition(".")
    return [int(n), int(s or 0)]
out["displays"] = [disp(n) for n in inp["displays"]]
out["positions"] = [list(dehulud.choose_position(x, y, [tuple(m) for m in mons], 180)) for x, y, mons in inp["positions"]]
out["quote"] = [shlex.quote(s) for s in inp["quote"]]
out["configs"] = [dehulud.load_config(p) for p in inp["configs"]]
out["colors"] = [dehulud.bar_color(f) for f in inp["colors"]]
out["pe"] = [dehulud.pe_identity(bytes.fromhex(h)) for h in inp["pe"]]
out["maps"] = [dehulud.image_base(m) for m in inp["maps"]]
out["launch_option"] = dehulud.launch_option()
print(json.dumps(out))
"#;

fn s(v: &Value) -> &str {
    match v {
        Value::Str(s) => s,
        other => panic!("{other:?}"),
    }
}

fn list(v: &Value) -> &Vec<Value> {
    match v {
        Value::Array(a) => a,
        other => panic!("{other:?}"),
    }
}

fn opt_i(v: &Value) -> Option<i64> {
    match v {
        Value::Int(Some(n)) => Some(*n),
        Value::Null => None,
        other => panic!("{other:?}"),
    }
}

fn opt_f(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(Some(n)) => Some(*n as f64),
        Value::Null => None,
        other => panic!("{other:?}"),
    }
}

fn jf(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else {
        format!("{f:?}")
    }
}

fn jopt<T: ToString>(v: Option<T>) -> String {
    v.map_or("null".into(), |v| v.to_string())
}

fn header(ts: u32, size: u32, patch: Option<(usize, &[u8])>) -> Vec<u8> {
    let mut h = vec![0u8; 0x400];
    h[0..2].copy_from_slice(b"MZ");
    h[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    h[0x80..0x84].copy_from_slice(b"PE\0\0");
    h[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    h[0x88..0x8C].copy_from_slice(&ts.to_le_bytes());
    h[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes());
    h[0xD0..0xD4].copy_from_slice(&size.to_le_bytes());
    if let Some((at, bytes)) = patch {
        h[at..at + bytes.len()].copy_from_slice(bytes);
    }
    h
}

#[test]
fn same_texts_and_rules_as_the_python_version() {
    let dir = TempDir::new("pydiff");
    let idents = [(1700000000u32, 220008448u32), (0, 1), (4294967295, 4096), (1792504044, 1)];
    let readings: Vec<Reading> = vec![
        Reading::value("on foot", 42, 0.425),
        Reading::new("waiting for game"),
        Reading::value("vehicle", 100, 1.0),
        Reading::value("on foot", 0, 0.0),
        Reading::value("on foot", 99, 0.9999),
        Reading::with("no permission", "x", "right-click"),
        Reading::with("Unsupported game version", "d", "PE 1700000000"),
        Reading::with("not started by --launch", "d", "right-click"),
        Reading::new("on foot, no threshold"),
    ];
    let blobs = [
        0.0,
        0.5,
        29.0,
        42.5,
        99.99,
        100.0,
        150.0,
        1e6,
        1e6 + 1.0,
        -0.001,
        -0.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        1e-300,
    ];
    let thresholds = [0.0, -5.0, 1e-300, 0.001, 100.0, 500.0, 1100.0, 1e6, 1e6 + 1.0, f64::NAN, f64::INFINITY];
    let mut values = Vec::new();
    for b in blobs {
        for t in thresholds {
            values.push((if b > 40.0 { "vehicle" } else { "on foot" }, b, t));
        }
    }
    let displays = [
        ":0",
        ":1",
        ":12.1",
        "unix:0",
        "unix:3.2",
        "unix/:2",
        "unix/:2.0",
        ":007",
        ":99999999999",
        "192.0.2.1:12",
        "localhost:10",
        "localhost:0",
        "host:0",
        "[::1]:0",
        "::1:0",
        "tcp/:0",
        "inet/:0",
        "host/unix:0",
        "unix/host:0",
        "/tmp/.X11-unix/X0",
        ":",
        ":x",
        ":0.",
        ":0.0.0",
        " :0",
        ":0 ",
        ":0\n",
        "wayland-0",
        "",
        "unix",
        "unix:",
        ":١",
    ];
    let three = [(0, 0, 1440, 2560, false), (1440, 400, 3840, 2160, true), (5280, 1120, 2560, 1440, false)];
    type Mon = (i32, i32, i32, i32, bool);
    let positions: Vec<(Option<i32>, Option<i32>, Vec<Mon>)> = vec![
        (None, None, three.to_vec()),
        (Some(7636), Some(48), three.to_vec()),
        (Some(5076), Some(448), three.to_vec()),
        (Some(10), Some(10), three.to_vec()),
        (Some(6000), Some(1500), three.to_vec()),
        (Some(1420), Some(10), three.to_vec()),
        (Some(1421), Some(10), three.to_vec()),
        (None, None, vec![(0, 0, 1920, 1080, false), (1920, 0, 1920, 1080, false)]),
        (Some(5), None, vec![(0, 0, 2560, 1440, true)]),
        (Some(-100), Some(-100), vec![(-1920, -1080, 1920, 1080, false), (0, 0, 1280, 800, true)]),
    ];
    let quotes =
        ["/usr/bin/dehulud", "", "/a b/c", "/a/Bob's \"m\"/d", "a@b%c+d=e:f,g.h/i-j_k", "é", "$HOME", "a\nb", "*"];
    let configs = [
        "{",
        "[]",
        "null",
        "{\"x\": \"10\", \"y\": true, \"frame\": 1}",
        "{\"x\": 1e309}",
        "ÿþ",
        "{\"x\": 99999999999}",
        "{\"x\": 10, \"y\": \"a\", \"frame\": true, \"extra\": 1}",
        "{\"x\": 10.0, \"y\": -99999}",
        "{\"x\": -0, \"y\": 99999, \"frame\": false}",
        "{\"x\": 1, \"x\": 2, \"frame\": true, \"frame\": 0}",
        " {\"y\": 5}\n",
        "{\"x\": 5} x",
        "{\"x\": NaN, \"y\": Infinity}",
        "{\"x\": 100000, \"y\": -100000}",
        "{\"a\\u00e9\": [1, {\"b\": null}], \"y\": 7}",
        "\"text\"",
        "{\"x\": 1,}",
        "",
        "{\"x\": 3, \"frame\": \"true\"}",
        "{\"x\": 01}",
        "{\"x\": 1E2, \"y\": 12}",
        "[{\"x\": 1}]",
    ];
    let mut config_paths = Vec::new();
    for (i, c) in configs.iter().enumerate() {
        let p = dir.path().join(format!("c{i}.json"));
        std::fs::write(&p, c).unwrap();
        config_paths.push(p);
    }
    let deep = dir.path().join("deep.json");
    std::fs::write(&deep, "[".repeat(100000)).unwrap();
    config_paths.push(deep);
    let bad_utf8 = dir.path().join("bad.json");
    std::fs::write(&bad_utf8, b"\xff\xfe{}").unwrap();
    config_paths.push(bad_utf8);
    config_paths.push(dir.path().join("missing.json"));
    let colors = [None, Some(0.0), Some(0.49), Some(0.4999999), Some(0.5), Some(0.74), Some(0.75), Some(1.0)];
    let pes: Vec<Vec<u8>> = vec![
        header(1792504044, 220008448, None),
        header(1700000000, 220008448, None),
        header(1, 2, Some((0, b"ZM"))),
        header(1, 2, Some((0x80, b"PX"))),
        header(1, 2, Some((0x84, b"\x4c\x01"))),
        header(1, 2, Some((0x98, b"\x0b\x01"))),
        header(1, 2, Some((0x3C, b"\xf0\x03"))),
        header(1, 2, Some((0x3C, b"\xa8\x03"))),
        header(1792504044, 220008448, None)[..0xD8].to_vec(),
        header(1792504044, 220008448, None)[..0xD7].to_vec(),
        b"MZ".to_vec(),
    ];
    let maps = [
        "100000-200000 r--p 00000000 08:01 5 /x/DuneSandbox-Win64-Shipping.exe.bak\n140001000-140100000 r-xp 00001000 08:01 9 /g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n140000000-140001000 r--p 00000000 08:01 9 /g/DUNE Awakening/DuneSandbox-Win64-Shipping.exe\n7f0000000000-7f0000001000 r--p 00000000 08:01 9 /g/x/DuneSandbox-Win64-Shipping.exe (deleted)\ngarbage line\n",
        "140000000-140001000 r--p 00001000 08:01 9 /g/DuneSandbox-Win64-Shipping.exe\n",
        "7f00-7f01 r--p 00000000 08:01 9          /g/x/DuneSandbox-Win64-Shipping.exe (deleted)\n",
        "zz-7f01 r--p 00000000 08:01 9 /g/DuneSandbox-Win64-Shipping.exe\n1000-2000 r--p 0000000g 08:01 9 /g/DuneSandbox-Win64-Shipping.exe\n",
        "3000-4000 r--p 00000000 08:01 9 DuneSandbox-Win64-Shipping.exe\n2000-3000 r--p 00000000 08:01 9 /a b/DuneSandbox-Win64-Shipping.exe\n",
        "",
    ];

    // the same inputs for both
    let arr = |items: Vec<String>| format!("[{}]", items.join(", "));
    let input = format!(
        "{{\"idents\": {}, \"readings\": {}, \"values\": {}, \"displays\": {}, \"positions\": {}, \"quote\": {}, \"configs\": {}, \
         \"colors\": {}, \"pe\": {}, \"maps\": {}}}",
        arr(idents.iter().map(|(t, s)| format!("[{t}, {s}]")).collect()),
        arr(readings
            .iter()
            .map(|r| format!(
                "[{}, {}, {}, {}, {}]",
                quote(&r.status),
                jopt(r.percent),
                r.fraction.map_or("null".into(), jf),
                quote(&r.detail),
                quote(&r.hint)
            ))
            .collect()),
        arr(values.iter().map(|(m, b, t)| format!("[{}, {}, {}]", quote(m), jf(*b), jf(*t))).collect()),
        arr(displays.iter().map(|d| quote(d)).collect()),
        arr(positions
            .iter()
            .map(|(x, y, mons)| {
                let ms: Vec<String> = mons.iter().map(|(a, b, c, d, e)| format!("[{a}, {b}, {c}, {d}, {e}]")).collect();
                format!("[{}, {}, {}]", jopt(*x), jopt(*y), arr(ms))
            })
            .collect()),
        arr(quotes.iter().map(|q| quote(q)).collect()),
        arr(config_paths.iter().map(|p| quote(&p.to_string_lossy())).collect()),
        arr(colors.iter().map(|c| c.map_or("null".into(), jf)).collect()),
        arr(pes.iter().map(|h| quote(&h.iter().map(|b| format!("{b:02x}")).collect::<String>())).collect()),
        arr(maps.iter().map(|m| quote(m)).collect()),
    );
    let mut child = Command::new(python())
        .args(["-B", "-c", SCRIPT])
        .arg(repo())
        .arg(PROG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let py = json::parse(&String::from_utf8(out.stdout).unwrap()).unwrap();
    let get = |k: &str| list(py.get(k).unwrap()).clone();
    for (k, n) in [
        ("help", 12),
        ("outside", 4),
        ("unsupported", idents.len()),
        ("lines", readings.len()),
        ("values", values.len()),
        ("displays", displays.len()),
        ("positions", positions.len()),
        ("quote", quotes.len()),
        ("configs", config_paths.len()),
        ("colors", colors.len()),
        ("pe", pes.len()),
        ("maps", maps.len()),
    ] {
        assert_eq!(get(k).len(), n, "{k}");
    }

    // help texts, notes, unsupported message
    let mut i = 0;
    for scope in [Some(0), Some(1), Some(2), Some(3), None, Some(7)] {
        for launched in [false, true] {
            assert_eq!(help::permission_help(scope, launched, PROG), s(&get("help")[i]), "{scope:?} {launched}");
            i += 1;
        }
    }
    for (i, scope) in [1, 2, 3, 7].iter().enumerate() {
        assert_eq!(help::outside_launch_help(4242, *scope, PROG), s(&get("outside")[i]), "{scope}");
    }
    assert_eq!(help::outside_note(9, Some(0)), s(&get("note")[0]));
    assert_eq!(help::outside_note(9, None), s(&get("note")[1]));
    for (i, ident) in idents.iter().enumerate() {
        assert_eq!(help::unsupported(*ident), s(&get("unsupported")[i]));
    }
    assert_eq!(help::launch_option(PROG), s(py.get("launch_option").unwrap()));
    // terminal line
    for (i, r) in readings.iter().enumerate() {
        assert_eq!(format_line(r), s(&get("lines")[i]), "{r:?}");
    }
    // value rules
    for (i, (m, b, t)) in values.iter().enumerate() {
        let want = list(&get("values")[i]).clone();
        let got = meter::reading_for(m, *b, *t);
        assert_eq!(got.status, s(&want[0]), "{b} {t}");
        assert_eq!(got.percent.map(i64::from), opt_i(&want[1]), "{b} {t}");
        assert_eq!(got.fraction, opt_f(&want[2]), "{b} {t}");
    }
    // display names
    for (i, d) in displays.iter().enumerate() {
        let want = match &get("displays")[i] {
            Value::Null => None,
            v => Some((opt_i(&list(v)[0]).unwrap() as u64, opt_i(&list(v)[1]).unwrap() as u32)),
        };
        assert_eq!(LocalDisplay::parse(d).map(|l| (l.number, l.screen)), want, "{d:?}");
    }
    // placement
    for (i, (x, y, mons)) in positions.iter().enumerate() {
        let ms: Vec<Monitor> =
            mons.iter().map(|&(x, y, width, height, primary)| Monitor { x, y, width, height, primary }).collect();
        let want = list(&get("positions")[i]).iter().map(|v| opt_i(v).unwrap() as i32).collect::<Vec<_>>();
        let got = config::choose_position(*x, *y, &ms, 180);
        assert_eq!(vec![got.0, got.1], want, "{x:?} {y:?}");
    }
    // quoting
    for (i, q) in quotes.iter().enumerate() {
        assert_eq!(help::shell_quote(q), s(&get("quote")[i]), "{q:?}");
    }
    // config files
    for (i, p) in config_paths.iter().enumerate() {
        let want = &get("configs")[i];
        let c = config::load_config(p);
        let got = (c.x.map(i64::from), c.y.map(i64::from), c.frame);
        let want = (
            opt_i(want.get("x").unwrap()),
            opt_i(want.get("y").unwrap()),
            want.get("frame") == Some(&Value::Bool(true)),
        );
        assert_eq!(
            got,
            want,
            "{}",
            std::fs::read_to_string(p).unwrap_or_default().chars().take(40).collect::<String>()
        );
    }
    // bar colours
    for (i, c) in colors.iter().enumerate() {
        assert_eq!(hex(bar_color(*c)), s(&get("colors")[i]), "{c:?}");
    }
    // PE identity
    for (i, h) in pes.iter().enumerate() {
        let want = match &get("pe")[i] {
            Value::Null => None,
            v => Some((opt_i(&list(v)[0]).unwrap() as u32, opt_i(&list(v)[1]).unwrap() as u32)),
        };
        assert_eq!(meter::pe_identity(h), want, "header {i}");
    }
    // maps
    for (i, m) in maps.iter().enumerate() {
        let want = match &get("maps")[i] {
            Value::Null => None,
            v => Some((opt_i(&list(v)[0]).unwrap() as u64, s(&list(v)[1]).to_string())),
        };
        assert_eq!(meter::image_base(m), want, "maps {i}");
    }
}
