//! DeHulud Combat's config file: `$XDG_CONFIG_HOME/dehulud-combat/config.json` (its own folder, by the same
//! rules as DeHulud's, shared with the Python version of DeHulud Combat: same file, same format). The window
//! position and frame flag as in DeHulud, plus the three section switches.

use crate::json::{self, Value};
use crate::sections::{Feature, Features};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub use crate::config::{Monitor, choose_position, home};

/// Window position (None until the user moved it), the frame flag and the three section switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub frame: bool,
    /// Keys `worm`, `damage_taken`, `damage_dealt`.
    pub sections: Features,
}

pub const DEFAULTS: Config = Config { x: None, y: None, frame: false, sections: Features::ALL };

/// `$XDG_CONFIG_HOME/dehulud-combat/config.json`, by the rules of [`crate::config::config_path_from`].
pub fn config_path_from(xdg: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    crate::config::config_path_in("dehulud-combat", xdg, home)
}

pub fn config_path() -> Option<PathBuf> {
    config_path_from(std::env::var_os("XDG_CONFIG_HOME").as_deref(), home().as_deref())
}

/// Window position and frame flag by DeHulud's rules, and the three section switches; anything missing or
/// malformed falls back to the defaults (each key on its own).
pub fn load_config(path: &Path) -> Config {
    let plain = crate::config::load_config(path);
    let mut cfg = Config { x: plain.x, y: plain.y, frame: plain.frame, ..DEFAULTS };
    let Ok(text) = fs::read_to_string(path) else { return cfg };
    let Ok(data) = json::parse(&text) else { return cfg };
    for f in Feature::ALL {
        if let Some(Value::Bool(b)) = data.get(f.key()) {
            cfg.sections.set(f, *b);
        }
    }
    cfg
}

/// The file's text, exactly as Python's `json.dump` writes it.
pub fn to_json(cfg: &Config) -> String {
    let n = |v: Option<i32>| v.map_or("null".to_string(), |v| v.to_string());
    let keys: Vec<String> =
        Feature::ALL.iter().map(|f| format!(", \"{}\": {}", f.key(), cfg.sections.has(*f))).collect();
    format!("{{\"x\": {}, \"y\": {}, \"frame\": {}{}}}", n(cfg.x), n(cfg.y), cfg.frame, keys.concat())
}

/// Writes the config atomically (a temporary file renamed over the old one); false when that fails.
pub fn save_config(path: &Path, cfg: &Config) -> bool {
    crate::config::save_text(path, &to_json(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "dehulud-unit-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn path() {
        let home = Some(Path::new("/h"));
        let p = |s: &str| Some(PathBuf::from(s));
        assert_eq!(config_path_from(Some(OsStr::new("/x/cfg")), home), p("/x/cfg/dehulud-combat/config.json"));
        assert_eq!(config_path_from(None, home), p("/h/.config/dehulud-combat/config.json"));
        assert_eq!(config_path_from(Some(OsStr::new("relative")), home), p("/h/.config/dehulud-combat/config.json"));
        assert_eq!(config_path_from(None, None), None);
    }

    #[test]
    fn missing_and_bad_configs_give_defaults() {
        let dir = tmpdir("cfg-bad");
        let path = dir.join("dehulud").join("config.json");
        assert_eq!(load_config(&path), DEFAULTS);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let deep = "[".repeat(100000);
        for bad in [
            "{",
            "[]",
            "null",
            "{\"x\": \"10\", \"y\": true, \"frame\": 1}",
            "{\"x\": 1e309}",
            "\u{ff}\u{fe}",
            deep.as_str(),
            "{\"x\": 99999999999}",
            "{\"x\": 10.0, \"y\": 100000}",
            "{\"x\": 5} x",
        ] {
            fs::write(&path, bad).unwrap();
            assert_eq!(load_config(&path), DEFAULTS, "{}", &bad[..bad.len().min(20)]);
        }
        fs::write(&path, b"\xff\xfe{}").unwrap(); // not UTF-8
        assert_eq!(load_config(&path), DEFAULTS);
        fs::write(&path, "{\"x\": 10, \"y\": \"a\", \"frame\": true, \"extra\": 1}").unwrap();
        assert_eq!(load_config(&path), Config { x: Some(10), frame: true, ..DEFAULTS });
        fs::write(&path, "{\"x\": -99999, \"y\": 99999}").unwrap();
        assert_eq!(load_config(&path), Config { x: Some(-99999), y: Some(99999), ..DEFAULTS });
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_and_load() {
        let dir = tmpdir("cfg-save");
        let path = dir.join("dehulud").join("config.json");
        let cfg = Config { x: Some(12), y: Some(34), frame: true, ..DEFAULTS };
        assert!(save_config(&path, &cfg));
        assert_eq!(load_config(&path), cfg);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        let on = "\"worm\": true, \"damage_taken\": true, \"damage_dealt\": true";
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{{\"x\": 12, \"y\": 34, \"frame\": true, {on}}}"));
        assert!(save_config(&path, &DEFAULTS));
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{{\"x\": null, \"y\": null, \"frame\": false, {on}}}"));
        let off = Config { sections: Features { worm: true, taken: false, dealt: false }, ..DEFAULTS };
        assert!(save_config(&path, &off));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"x\": null, \"y\": null, \"frame\": false, \"worm\": true, \"damage_taken\": false, \"damage_dealt\": false}"
        );
        assert_eq!(load_config(&path), off);
        assert!(!dir.join("dehulud").join("config.json.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn section_switches() {
        let dir = tmpdir("cfg-sections");
        let path = dir.join("config.json");
        fs::write(&path, "{\"x\": 5, \"y\": 6}").unwrap(); // a file from before the switches existed
        assert_eq!(load_config(&path), Config { x: Some(5), y: Some(6), ..DEFAULTS });
        fs::write(&path, "{\"worm\": false, \"damage_taken\": true, \"damage_dealt\": false}").unwrap();
        assert_eq!(load_config(&path).sections, Features { worm: false, taken: true, dealt: false });
        for bad in ["\"false\"", "0", "1", "null", "[]", "{}", "\"no\""] {
            fs::write(&path, format!("{{\"worm\": {bad}, \"damage_taken\": false, \"damage_dealt\": {bad}}}")).unwrap();
            assert_eq!(load_config(&path).sections, Features { worm: true, taken: false, dealt: true }, "{bad}");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_never_crashes() {
        let dir = tmpdir("cfg-block");
        let blocker = dir.join("file");
        fs::write(&blocker, "").unwrap();
        assert!(!save_config(&blocker.join("dehulud").join("config.json"), &Config { x: Some(1), ..DEFAULTS }));
        let _ = fs::remove_dir_all(dir);
    }
}
