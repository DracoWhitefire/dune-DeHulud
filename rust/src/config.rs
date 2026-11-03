//! The window's config file (`$XDG_CONFIG_HOME/dehulud/config.json`, shared with the Python version: same
//! file, same format) and the window placement rule.

use crate::json::{self, Value};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// Window position (None until the user moved it) and the frame flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub frame: bool,
}

pub const DEFAULTS: Config = Config { x: None, y: None, frame: false };

/// The user's home folder from $HOME, trailing slashes removed (an empty $HOME is "/"), like Python's
/// `os.path.expanduser("~")` when HOME is set. None without $HOME: there is deliberately no password-database
/// fallback, because that lookup can ask a local name service over a socket (nscd, systemd-userdb) and the
/// meter opens no socket other than its X display.
pub fn home() -> Option<PathBuf> {
    let h = std::env::var_os("HOME")?;
    let s = h.to_string_lossy();
    let trimmed = s.trim_end_matches('/');
    Some(PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed }))
}

/// `$XDG_CONFIG_HOME/dehulud/config.json`; ~/.config when XDG_CONFIG_HOME is unset or not absolute. None when
/// neither an absolute XDG_CONFIG_HOME nor a home folder is known: the window then keeps no position.
pub fn config_path_from(xdg: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    config_path_in("dehulud", xdg, home)
}

/// `config.json` in the folder `folder` of the config base, by the same rules (DeHulud Combat has its own).
pub fn config_path_in(folder: &str, xdg: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let base = match xdg {
        Some(x) if Path::new(x).is_absolute() => PathBuf::from(x),
        _ => home?.join(".config"),
    };
    Some(base.join(folder).join("config.json"))
}

pub fn config_path() -> Option<PathBuf> {
    config_path_from(std::env::var_os("XDG_CONFIG_HOME").as_deref(), home().as_deref())
}

/// Window position and frame flag; anything missing or malformed falls back to the defaults.
pub fn load_config(path: &Path) -> Config {
    let mut cfg = DEFAULTS;
    let Ok(text) = fs::read_to_string(path) else { return cfg };
    let Ok(data) = json::parse(&text) else { return cfg };
    if !matches!(data, Value::Object(_)) {
        return cfg;
    }
    let coord = |k: &str| match data.get(k) {
        Some(Value::Int(Some(v))) if -100000 < *v && *v < 100000 => Some(*v as i32),
        _ => None,
    };
    cfg.x = coord("x");
    cfg.y = coord("y");
    if let Some(Value::Bool(b)) = data.get("frame") {
        cfg.frame = *b;
    }
    cfg
}

/// The file's text, exactly as Python's `json.dump` writes it.
pub fn to_json(cfg: &Config) -> String {
    let n = |v: Option<i32>| v.map_or("null".to_string(), |v| v.to_string());
    format!("{{\"x\": {}, \"y\": {}, \"frame\": {}}}", n(cfg.x), n(cfg.y), cfg.frame)
}

/// Writes the config atomically (a temporary file renamed over the old one); false when that fails.
pub fn save_config(path: &Path, cfg: &Config) -> bool {
    save_text(path, &to_json(cfg))
}

/// Writes a config file's text atomically, creating its folder with mode 0700; false when that fails.
pub fn save_text(path: &Path, text: &str) -> bool {
    let Some(dir) = path.parent() else { return false };
    // like Python's os.makedirs(dir, mode=0o700, exist_ok=True): only the last folder gets mode 0700
    if let Some(up) = dir.parent() {
        let _ = fs::create_dir_all(up);
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(_) if dir.is_dir() => {}
        Err(_) => return false,
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, text).is_ok() && fs::rename(&tmp, path).is_ok()
}

/// One monitor of the X screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

/// Keep a saved position whose corner lies on a monitor; otherwise the top-right of the primary monitor (the
/// first monitor without a primary one).
pub fn choose_position(x: Option<i32>, y: Option<i32>, mons: &[Monitor], width: i32) -> (i32, i32) {
    if let (Some(x), Some(y)) = (x, y) {
        for m in mons {
            if m.x <= x && x <= m.x + m.width - 20 && m.y <= y && y <= m.y + m.height - 20 {
                return (x, y);
            }
        }
    }
    let m = mons.iter().find(|m| m.primary).or(mons.first()).copied().unwrap_or(Monitor {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        primary: true,
    });
    (m.x + m.width - width - 24, m.y + 48)
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
        assert_eq!(config_path_from(Some(OsStr::new("/x/cfg")), home), p("/x/cfg/dehulud/config.json"));
        assert_eq!(config_path_from(None, home), p("/h/.config/dehulud/config.json"));
        assert_eq!(config_path_from(Some(OsStr::new("relative")), home), p("/h/.config/dehulud/config.json"));
        assert_eq!(config_path_from(Some(OsStr::new("")), home), p("/h/.config/dehulud/config.json"));
        // without a home folder: an absolute XDG_CONFIG_HOME still works, otherwise no config file at all
        assert_eq!(config_path_from(Some(OsStr::new("/x/cfg")), None), p("/x/cfg/dehulud/config.json"));
        assert_eq!(config_path_from(None, None), None);
        assert_eq!(config_path_from(Some(OsStr::new("relative")), None), None);
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
        assert_eq!(load_config(&path), Config { x: Some(10), y: None, frame: true });
        fs::write(&path, "{\"x\": -99999, \"y\": 99999}").unwrap();
        assert_eq!(load_config(&path), Config { x: Some(-99999), y: Some(99999), frame: false });
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_and_load() {
        let dir = tmpdir("cfg-save");
        let path = dir.join("dehulud").join("config.json");
        let cfg = Config { x: Some(12), y: Some(34), frame: true };
        assert!(save_config(&path, &cfg));
        assert_eq!(load_config(&path), cfg);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"x\": 12, \"y\": 34, \"frame\": true}");
        assert!(save_config(&path, &DEFAULTS));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"x\": null, \"y\": null, \"frame\": false}");
        assert!(!dir.join("dehulud").join("config.json.tmp").exists());
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

    fn mon(x: i32, y: i32, width: i32, height: i32, primary: bool) -> Monitor {
        Monitor { x, y, width, height, primary }
    }

    // Three monitors: a portrait one at the left, the primary 4K one lower in the middle, a 1440p one lower right.
    fn three() -> Vec<Monitor> {
        vec![mon(0, 0, 1440, 2560, false), mon(1440, 400, 3840, 2160, true), mon(5280, 1120, 2560, 1440, false)]
    }

    #[test]
    fn placement() {
        assert_eq!(choose_position(None, None, &three(), 180), (1440 + 3840 - 180 - 24, 400 + 48));
        // the old default in the dead zone of this layout is moved onto a monitor
        assert_eq!(choose_position(Some(7636), Some(48), &three(), 180), (5076, 448));
        for (x, y) in [(5076, 448), (10, 10), (6000, 1500)] {
            assert_eq!(choose_position(Some(x), Some(y), &three(), 180), (x, y));
        }
        let two = [mon(0, 0, 1920, 1080, false), mon(1920, 0, 1920, 1080, false)];
        assert_eq!(choose_position(None, None, &two, 180), (1920 - 180 - 24, 48));
        assert_eq!(choose_position(None, None, &[mon(0, 0, 2560, 1440, true)], 180), (2560 - 204, 48));
        assert_eq!(choose_position(Some(5), None, &[mon(0, 0, 2560, 1440, true)], 180), (2356, 48));
    }
}
