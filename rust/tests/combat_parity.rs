//! Differential test of DeHulud Combat against its reference implementation, `dehulud_combat.py` (imported,
//! never run as a meter): the window's sections and the damage display, and the config file with its section
//! switches, must come out the same in both versions. Needs a Python 3 (see common::python).

mod common;

use common::*;
use dehulud::combat_config;
use dehulud::json::{self, Value, quote};
use dehulud::sections::{self, DamageView, Feature, Features};
use dehulud::window::hex;
use std::process::{Command, Stdio};

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

const SECTIONS_SCRIPT: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
import dehulud_combat as dehulud
inp = json.load(sys.stdin)
out = {}
combos = [frozenset(c) for c in inp["combos"]]
out["layouts"] = [[[[n, y] for n, y in dehulud.layout(c)[0]], dehulud.layout(c)[1]] for c in combos]
out["headers"] = [[list(h) for h in dehulud.header_cells(c)] for c in combos]
out["zones"] = [dehulud.header_zone(x, y) for x, y in inp["points"]]
out["clicks"] = [dehulud.is_click(dx, dy) for dx, dy in inp["moves"]]
out["numbers"] = [dehulud.shown_number(v) for v in inp["numbers"]]
view, cells = dehulud.DamageView(), []
for st in inp["steps"]:
    on = frozenset(st["on"])
    view.step(dehulud.Reading("on foot", taken=st["taken"], dealt=st["dealt"]), on)
    cells.append([list(c) for c in dehulud.damage_cells(view, on)])
out["cells"] = cells
print(json.dumps(out))
"#;

/// The window's sections and the damage display as pure functions: layout, header switches and click zones,
/// the click-or-drag rule, rounding and cap, and a run of polls through the damage columns (order, at most four,
/// fade by age, removal on the fifth poll, '-', a section switched off) must come out the same in both versions.
#[test]
fn same_sections_and_damage_display_as_the_python_version() {
    let combos: Vec<Features> =
        (0..8).map(|i| Features { worm: i & 1 != 0, taken: i & 2 != 0, dealt: i & 4 != 0 }).collect();
    let keys = |f: Features| format!("[{}]", f.keys().iter().map(|k| quote(k)).collect::<Vec<_>>().join(", "));
    let mut points = Vec::new();
    for y in [-1, 0, 9, 17, 18, 40] {
        for x in [-1, 0, 5, 56, 57, 90, 116, 117, 150, 179, 180] {
            points.push((x, y));
        }
    }
    let moves: Vec<(i32, i32)> = (-4..=4).flat_map(|dx| (-4..=4).map(move |dy| (dx, dy))).collect();
    let numbers = [0.0, 0.2, 0.49, 0.5, 1.5, 2.5, 36.6, 99998.5, 99999.4, 99999.5, 123456.0, 1e300, -0.4, -1.0];
    // (taken, dealt, sections on); taken: None = unreadable, Some(None) = nothing; dealt likewise per column
    type Step = (Option<Option<f64>>, Option<(Option<f64>, Option<f64>)>, Features);
    let all = Features::ALL;
    let no_taken = Features { taken: false, ..all };
    let steps: Vec<Step> = vec![
        (Some(None), Some((None, None)), all),
        (Some(Some(37.0)), Some((Some(120.0), None)), all),
        (Some(Some(12.4)), Some((Some(84.0), Some(40.0))), all),
        (Some(None), Some((None, None)), all),
        (Some(Some(0.3)), Some((Some(5.0), Some(1e9))), all),
        (Some(Some(9.0)), Some((Some(6.0), None)), all),
        (Some(Some(150000.0)), Some((None, Some(7.0))), all),
        (Some(None), Some((None, None)), all),
        (Some(None), Some((None, None)), all),
        (Some(None), Some((None, None)), all),
        (None, Some((Some(3.0), None)), all),
        (Some(Some(1.0)), None, all),
        (Some(Some(2.0)), Some((Some(4.0), Some(4.0))), no_taken),
        (Some(None), Some((None, None)), all),
        (Some(Some(8.0)), Some((None, None)), Features { dealt: false, ..all }),
        (Some(None), Some((None, None)), all),
    ];
    let num = |v: Option<f64>| v.map_or("[]".to_string(), |v| format!("[{}]", jf(v)));
    let step_json = |(t, d, on): &Step| {
        format!(
            "{{\"taken\": {}, \"dealt\": {}, \"on\": {}}}",
            t.map_or("null".to_string(), num),
            d.map_or("null".to_string(), |(h, s)| format!("[{}, {}]", num(h), num(s))),
            keys(*on)
        )
    };
    let input = format!(
        "{{\"combos\": [{}], \"points\": [{}], \"moves\": [{}], \"numbers\": [{}], \"steps\": [{}]}}",
        combos.iter().map(|c| keys(*c)).collect::<Vec<_>>().join(", "),
        points.iter().map(|(x, y)| format!("[{x}, {y}]")).collect::<Vec<_>>().join(", "),
        moves.iter().map(|(x, y)| format!("[{x}, {y}]")).collect::<Vec<_>>().join(", "),
        numbers.iter().map(|v| jf(*v)).collect::<Vec<_>>().join(", "),
        steps.iter().map(step_json).collect::<Vec<_>>().join(", ")
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let mut child = Command::new(python())
        .args(["-B", "-I", "-c", SECTIONS_SCRIPT, &root.to_string_lossy()])
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

    // the same structures from the Rust side, as JSON, compared value by value
    let rs = |text: String| json::parse(&text).unwrap();
    let opt_s = |v: Option<&str>| v.map_or("null".to_string(), quote);
    for (i, c) in combos.iter().enumerate() {
        let (secs, h) = sections::layout(*c);
        let l: Vec<String> = secs.iter().map(|(s, y)| format!("[{}, {y}]", quote(s.name()))).collect();
        assert_eq!(rs(format!("[[{}], {h}]", l.join(", "))), get("layouts")[i], "{c:?}");
        let hd: Vec<String> = sections::header_cells(*c)
            .iter()
            .map(|(x, t, rgb)| format!("[{x}, {}, {}]", quote(t), quote(&hex(*rgb))))
            .collect();
        assert_eq!(rs(format!("[{}]", hd.join(", "))), get("headers")[i], "{c:?}");
    }
    for (i, (x, y)) in points.iter().enumerate() {
        assert_eq!(rs(opt_s(sections::header_zone(*x, *y).map(Feature::key))), get("zones")[i], "{x} {y}");
    }
    for (i, (dx, dy)) in moves.iter().enumerate() {
        assert_eq!(Value::Bool(sections::is_click(*dx, *dy)), get("clicks")[i], "{dx} {dy}");
    }
    for (i, v) in numbers.iter().enumerate() {
        assert_eq!(rs(jopt(sections::shown_number(*v))), get("numbers")[i], "{v}");
    }
    let mut view = DamageView::default();
    for (i, (t, d, on)) in steps.iter().enumerate() {
        view.step(*t, *d, *on);
        let cells: Vec<String> = sections::damage_cells(&view, *on)
            .iter()
            .map(|(s, x, y, t, rgb)| format!("[{}, {x}, {y}, {}, {}]", quote(s.name()), quote(t), quote(&hex(*rgb))))
            .collect();
        assert_eq!(rs(format!("[{}]", cells.join(", "))), get("cells")[i], "poll {i}");
    }
}

const CONFIG_SCRIPT: &str = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
import dehulud_combat
inp = json.load(sys.stdin)
out = {"configs": [dehulud_combat.load_config(p) for p in inp["configs"]],
       "paths": [dehulud_combat.config_path(e) for e in inp["envs"]]}
print(json.dumps(out))
"#;

/// DeHulud Combat's config file: its own folder, and the same values read from the same text in both versions.
#[test]
fn same_config_as_the_python_version() {
    let dir = TempDir::new("combat-cfg");
    let texts = [
        "{",
        "[]",
        "{\"x\": 10, \"y\": \"a\", \"frame\": true, \"extra\": 1}",
        "{\"worm\": false, \"damage_taken\": true, \"damage_dealt\": false}",
        "{\"worm\": 0, \"damage_taken\": \"false\", \"damage_dealt\": null, \"frame\": true}",
        "{\"x\": 4, \"damage_dealt\": false, \"damage_dealt\": true, \"worm\": false}",
        "{\"x\": 99999999999, \"worm\": false}",
    ];
    let paths: Vec<std::path::PathBuf> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let p = dir.path().join(format!("c{i}.json"));
            std::fs::write(&p, t).unwrap();
            p
        })
        .collect();
    let envs = [("/x/cfg", "/h"), ("", "/h"), ("relative", "/h//")];
    let input = format!(
        "{{\"configs\": [{}], \"envs\": [{}]}}",
        paths.iter().map(|p| quote(&p.to_string_lossy())).collect::<Vec<_>>().join(", "),
        envs.iter()
            .map(|(x, h)| format!("{{\"XDG_CONFIG_HOME\": {}, \"HOME\": {}}}", quote(x), quote(h)))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let mut child = Command::new(python())
        .args(["-B", "-I", "-c", CONFIG_SCRIPT, &root.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let py = json::parse(&String::from_utf8(out.stdout).unwrap()).unwrap();
    for (i, p) in paths.iter().enumerate() {
        let c = combat_config::load_config(p);
        let got = json::parse(&combat_config::to_json(&c)).unwrap();
        assert_eq!(got, list(py.get("configs").unwrap())[i], "{}", texts[i]);
    }
    for (i, (x, h)) in envs.iter().enumerate() {
        let xdg = Some(std::ffi::OsStr::new(x));
        let got = combat_config::config_path_from(xdg, Some(std::path::Path::new(h.trim_end_matches('/'))));
        let want = s(&list(py.get("paths").unwrap())[i]).to_string();
        assert_eq!(got.unwrap().to_string_lossy(), want, "{x} {h}");
    }
}
