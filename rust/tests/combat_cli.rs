//! DeHulud Combat's command line (the real `dehulud-combat` binary), and that DeHulud has none of it.
//!
//! Runs that reach the meter use the test build (`--features test-source`) with a control file that lists no
//! process at all or names the test's own process, so no test ever scans the real /proc.

mod common;

use common::*;
use std::process::Command;

fn quiet(cmd: &mut Command, dir: &TempDir) -> std::process::Output {
    let ctl = dir.path().join("control");
    if !ctl.exists() {
        std::fs::write(&ctl, "").unwrap(); // a test build lists no process at all
    }
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("DEHULUD_TEST_SOURCE", ctl)
        .output()
        .unwrap()
}

#[test]
fn combat_has_its_own_help_and_dehulud_has_no_section_flags() {
    let dir = TempDir::new("combat-cli");
    let out = quiet(Command::new(combat_bin()).arg("--help"), &dir);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("DeHulud Combat - sandworm threat meter"), "{text}");
    for flag in ["--no-worm", "--no-taken", "--no-dealt", "dehulud-combat --launch %command%"] {
        assert!(text.contains(flag), "{flag}");
    }
    let out = quiet(Command::new(combat_bin()).arg("--bogus"), &dir);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "dehulud-combat: unknown option '--bogus' (try --help)\n");
    // DeHulud itself is unchanged: no section flags, nothing about them in its help
    let out = quiet(Command::new(bin()).arg("--help"), &dir);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("--no-"));
    for flag in ["--no-worm", "--no-taken", "--no-dealt"] {
        let out = quiet(Command::new(bin()).arg(flag), &dir);
        assert_eq!(out.status.code(), Some(1), "{flag}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), format!("dehulud: unknown option '{flag}' (try --help)\n"));
    }
}

/// Text mode shows only the worm meter: with --no-worm it has nothing to show and must not read. A usage error
/// before any poll; the damage flags change nothing; with --launch the game still starts, without a meter.
#[cfg(feature = "test-source")]
#[test]
fn text_mode_refuses_no_worm_without_reading() {
    let msg = "dehulud-combat: text mode shows only the worm meter; drop --no-worm\n";
    let dir = TempDir::new("noworm");
    let trace = dir.path().join("trace.jsonl");
    // the test's own process listed as the game: any attach or poll would show in the trace
    std::fs::write(
        dir.path().join("control"),
        format!("pid={}\nbase=4096\npoll_ms=20\ntrace={}\n", std::process::id(), trace.display()),
    )
    .unwrap();
    let polls = || std::fs::read_to_string(&trace).unwrap_or_default().matches("\"event\": \"poll\"").count();
    for argv in [&["--text", "--no-worm"][..], &["--no-dealt", "--no-worm", "--text"]] {
        let out = quiet(Command::new(combat_bin()).args(argv), &dir);
        assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stderr).as_ref()), (Some(1), msg), "{argv:?}");
        assert!(out.stdout.is_empty());
    }
    // no X display: the window falls back to text mode, which has nothing to show either
    let out = quiet(Command::new(combat_bin()).arg("--no-worm"), &dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).ends_with(msg), "{}", String::from_utf8_lossy(&out.stderr));
    // with --launch the game is never blocked: it runs, its exit status is DeHulud's, and no meter runs
    let out =
        quiet(Command::new(combat_bin()).args(["--text", "--no-worm", "--launch", "/bin/sh", "-c", "exit 3"]), &dir);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("drop --no-worm (the game starts without the meter)"));
    assert_eq!(polls(), 0, "no poll in any of these runs");
    // the same run with the damage flags instead: they change nothing, the worm line is polled until the game ends
    let out = quiet(
        Command::new(combat_bin()).args([
            "--text",
            "--no-taken",
            "--no-dealt",
            "--launch",
            "/bin/sh",
            "-c",
            "sleep 0.3; exit 3",
        ]),
        &dir,
    );
    assert_eq!(out.status.code(), Some(3));
    assert!(polls() > 0, "the control file reaches the meter");
}
