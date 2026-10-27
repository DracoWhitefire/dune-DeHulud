//! The texts DeHulud prints: permission help per ptrace_scope value, the --launch help, the unsupported-build
//! message, and the shell-quoted launch line (every printed command quotes the program's own path).

use crate::meter::BUILDS;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The real path of this program (symlinks resolved), as printed in the launch commands.
pub fn program() -> String {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .unwrap_or_else(|_| PathBuf::from("dehulud"))
            .to_string_lossy()
            .into_owned()
    })
    .clone()
}

/// Quotes `s` for a POSIX shell exactly like Python's `shlex.quote`: unchanged when it only holds
/// `[A-Za-z0-9_@%+=:,./-]`, otherwise in single quotes with each `'` written as `'"'"'`.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// The Steam launch option for this program. The path is quoted for the shell: a path with a space or a quote
/// in it would otherwise be split into several words and the game would not start.
pub fn launch_option(program: &str) -> String {
    format!("{} --launch %command%", shell_quote(program))
}

/// The exact fix for the user's ptrace_scope value (kernel.yama.ptrace_scope). Yama's values: 0 adds no
/// restriction of its own (the normal same-user rules still apply), 1 allows a program to read only its own
/// descendants, 2 only programs with CAP_SYS_PTRACE, 3 nobody, and 3 cannot be lowered until a reboot.
pub fn permission_help(scope: Option<i64>, launched: bool, program: &str) -> String {
    let shown = scope.map_or("not readable".to_string(), |s| s.to_string());
    let head = format!("DeHulud was refused permission to read the game's memory (ptrace_scope is {shown}).\n");
    let fix = match scope {
        Some(0) => "ptrace_scope 0 adds no restriction of its own, but the normal rules still apply: the game must run as \
                    the same user as DeHulud and not in a sandbox (Flatpak Steam), must not have made itself unreadable, \
                    and no security module may block it (SELinux: 'getsebool deny_ptrace' must be off)."
            .to_string(),
        Some(1) if launched => "With 1 a program may read its own descendants, and DeHulud started the game through the \
                                --launch wrapper, but it is still not readable: your Steam setup probably runs it in a \
                                container or sandbox that prevents it. Use instead (until reboot):\n  sudo sysctl \
                                kernel.yama.ptrace_scope=0\nand start DeHulud by itself, without the launch option."
            .to_string(),
        Some(1) => format!(
            "With 1 a program may read only its own descendants (the programs it started, and theirs). Fix, pick one:\n  \
             1. Steam > Dune: Awakening > Properties > Launch Options:\n       {}\n  \
             2. Allow it until the next reboot:  sudo sysctl kernel.yama.ptrace_scope=0\n     \
             (trade-off: then every program running as your user can read your other programs)",
            launch_option(program)
        ),
        Some(2) => format!(
            "With 2 only a program with the CAP_SYS_PTRACE capability (normally only root) may read another program. \
             Lower it to 1 and use the Steam launch option ({}), or to 0:\n  sudo sysctl kernel.yama.ptrace_scope=1",
            launch_option(program)
        ),
        Some(3) => "With 3 reading other programs is switched off, and 3 cannot be lowered while the system runs. Change \
                    kernel.yama.ptrace_scope in the file under /etc/sysctl.d/ that sets 3, then reboot."
            .to_string(),
        _ => "The Yama setting /proc/sys/kernel/yama/ptrace_scope could not be read: Yama is not active, or the setting \
              is hidden (a container). Something else refuses the read: another user, a sandbox (Flatpak) or a \
              security module (SELinux 'deny_ptrace', AppArmor)."
            .to_string(),
    };
    format!("{head}{fix}\nDetails: README.md, 'Before you start: permission to read the game'.")
}

/// --launch mode, but the game is not a descendant of DeHulud: what works for this ptrace_scope value.
pub fn outside_launch_help(pid: i32, scope: i64, program: &str) -> String {
    let head = format!(
        "Steam started the game outside DeHulud (pid {pid} is not a descendant of this --launch DeHulud), so the launch \
         option cannot help, and ptrace_scope is {scope}.\n"
    );
    let trade = "Trade-off: with 0, every program running as your user can read the memory of your other programs \
                 (browser, password manager).";
    if scope == 3 {
        return format!(
            "{head}3 cannot be lowered while the system runs: change kernel.yama.ptrace_scope to 0 in the file under \
             /etc/sysctl.d/ that sets 3 (README, 'Change it permanently'), reboot, remove the launch option in Steam and \
             start DeHulud by itself. {trade}"
        );
    }
    let why = if scope == 2 {
        "With 2, only a program with the CAP_SYS_PTRACE capability (normally only root) may read another program, even \
         one it started.\n"
    } else {
        "With 1, a program may read only its own descendants (the programs it started, and theirs).\n"
    };
    format!(
        "{head}{why}What works: allow reading with\n  sudo sysctl kernel.yama.ptrace_scope=0      (until the next reboot)\n\
         then remove the launch option in Steam and start DeHulud by itself:  {}\n{trade}",
        shell_quote(program)
    )
}

/// Said with every reading of a game found outside --launch and read anyway.
pub fn outside_note(pid: i32, scope: Option<i64>) -> String {
    let shown = if scope == Some(0) { "0" } else { "not readable" };
    format!("DeHulud found the game outside --launch; reading it anyway (pid {pid}, ptrace_scope {shown}).")
}

/// The detail of "Unsupported game version".
pub fn unsupported(ident: (u32, u32)) -> String {
    let known: Vec<String> =
        BUILDS.iter().map(|b| format!("build {} (PE timestamp {})", b.build, b.pe_timestamp)).collect();
    format!(
        "Unsupported game version: PE timestamp {} ({} UTC), SizeOfImage {}.\nThis DeHulud knows: {}.\n\
         After a game update DeHulud shows '-' until a release with the new build's offsets exists.",
        ident.0,
        utc_minute(i64::from(ident.0)),
        ident.1,
        known.join(", ")
    )
}

/// `YYYY-MM-DD HH:MM` of a Unix time, in UTC.
pub fn utc_minute(t: i64) -> String {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // civil date from days since 1970-01-01 (proleptic Gregorian calendar)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// Python's `repr()` of a string, for messages that quote what the user typed (`'x'`, or `"it's"`).
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Option<i64>; 5] = [Some(0), Some(1), Some(2), Some(3), None];

    #[test]
    fn quoting_matches_shlex() {
        assert_eq!(shell_quote("/usr/local/bin/dehulud"), "/usr/local/bin/dehulud");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("/opt/x/My Tools/dehulud"), "'/opt/x/My Tools/dehulud'");
        assert_eq!(shell_quote("/a/Bob's \"m\"/d"), "'/a/Bob'\"'\"'s \"m\"/d'");
        assert_eq!(shell_quote("a@b%c+d=e:f,g.h/i-j_k"), "a@b%c+d=e:f,g.h/i-j_k");
        assert_eq!(shell_quote("é"), "'é'");
        assert_eq!(launch_option("/x y/dehulud"), "'/x y/dehulud' --launch %command%");
    }

    #[test]
    fn help_texts_differ_per_scope() {
        let texts: std::collections::HashSet<String> = ALL.iter().map(|s| permission_help(*s, false, "/p")).collect();
        assert_eq!(texts.len(), 5);
    }

    #[test]
    fn help_texts_describe_yama_exactly() {
        let t = |s| permission_help(s, false, "/p");
        assert!(t(Some(0)).contains("normal rules still apply"));
        for text in [t(Some(1)), permission_help(Some(1), true, "/p")] {
            assert!(text.contains("descendants"));
            assert!(!text.contains("parent"));
        }
        assert!(t(Some(2)).contains("CAP_SYS_PTRACE"));
        assert!(!t(Some(2)).contains("Only root"));
        assert!(t(Some(3)).contains("cannot be lowered while the system runs"));
        assert!(t(None).contains("could not be read"));
        assert!(!t(None).contains("There is no Yama"));
        for s in ALL {
            assert!(t(s).contains("ptrace_scope"));
        }
    }

    #[test]
    fn printed_commands_quote_the_path() {
        let path = "/t/My Tools/Bob's \"meters\"/dehulud";
        let quoted = format!("{} --launch %command%", shell_quote(path));
        for text in [permission_help(Some(1), false, path), permission_help(Some(2), false, path)] {
            assert!(text.contains(&quoted));
            assert!(!text.contains(path)); // never the bare path
        }
        let outside = outside_launch_help(77, 1, path);
        assert!(outside.contains(&shell_quote(path)) && !outside.contains(path));
    }

    #[test]
    fn outside_launch_texts() {
        let one = outside_launch_help(4242, 1, "/p");
        assert!(one.contains("sudo sysctl kernel.yama.ptrace_scope=0") && one.contains("remove the launch option"));
        assert!(one.contains("Trade-off") && one.contains("4242") && !one.contains("%command%"));
        let two = outside_launch_help(4242, 2, "/p");
        assert!(two.contains("CAP_SYS_PTRACE") && two.contains("sudo sysctl kernel.yama.ptrace_scope=0"));
        let three = outside_launch_help(4242, 3, "/p");
        assert!(three.contains("reboot") && three.contains("cannot be lowered"));
        assert!(!three.contains("sudo sysctl kernel.yama.ptrace_scope="));
        assert_eq!(
            outside_note(9, Some(0)),
            "DeHulud found the game outside --launch; reading it anyway (pid 9, ptrace_scope 0)."
        );
        assert!(outside_note(9, None).ends_with("ptrace_scope not readable)."));
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_minute(0), "1970-01-01 00:00");
        assert_eq!(utc_minute(1792504044), "2026-10-20 13:47");
        assert_eq!(utc_minute(1700000000), "2023-11-14 22:13");
        assert_eq!(utc_minute(951782400), "2000-02-29 00:00");
        assert_eq!(utc_minute(4294967295), "2106-02-07 06:28");
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(py_repr("--bogus"), "'--bogus'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(py_repr(":0\n"), "':0\\n'");
        assert_eq!(py_repr(""), "''");
    }
}
