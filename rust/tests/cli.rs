//! The command line and the text-mode fallbacks of the real binary.
//!
//! Runs that reach the meter use the test build (`--features test-source`) with a control file that lists no
//! process at all or scripts the readings, so no test ever scans the real /proc.

mod common;

use common::*;
use dehulud::help::shell_quote;
use std::process::Command;

/// The binary copied into a folder whose path needs quoting: "My Tools/Bob's "meters"".
fn awkward_copy(dir: &TempDir) -> String {
    let folder = dir.path().join("My Tools").join("Bob's \"meters\"");
    std::fs::create_dir_all(&folder).unwrap();
    let copy = folder.join("dehulud");
    // copied by another process: a file this test process held open for writing could be inherited by a child
    // that another test thread forks at that moment, and executing the copy would then fail ("text file busy")
    assert!(Command::new("cp").arg(bin()).arg(&copy).status().unwrap().success());
    std::fs::canonicalize(copy).unwrap().to_string_lossy().into_owned()
}

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
fn help_and_unknown_options() {
    let dir = TempDir::new("cli");
    let out = quiet(Command::new(bin()).arg("--help"), &dir);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("DeHulud - sandworm threat meter for Dune: Awakening on Linux."));
    assert!(text.contains("--launch %command%") && text.contains("process_vm_readv"));
    let out = quiet(Command::new(bin()).arg("--bogus"), &dir);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "dehulud: unknown option '--bogus' (try --help)\n");
}

#[test]
fn launch_without_a_command_prints_the_quoted_launch_option() {
    let dir = TempDir::new("quote");
    let path = awkward_copy(&dir);
    let out = quiet(Command::new(&path).arg("--launch"), &dir);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("{} --launch %command%", shell_quote(&path))), "{err}");
    assert!(!err.contains(&path), "never the bare path: {err}");
}

#[cfg(feature = "test-source")]
#[test]
fn the_printed_launch_option_runs_from_such_a_path() {
    let dir = TempDir::new("quote-run");
    let path = awkward_copy(&dir);
    let err = String::from_utf8_lossy(&quiet(Command::new(&path).arg("--launch"), &dir).stderr).into_owned();
    let line = err.trim().rsplit("Steam launch options: ").next().unwrap().to_string();
    // Steam replaces %command% with the game's command; here an inert recorder of its arguments.
    let receipt = dir.path().join("argv.txt");
    let recorder = dir.path().join("game");
    script(&recorder, &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", shell_quote(&receipt.to_string_lossy())));
    let line = line.replace("%command%", &format!("{} game-arg 'two words'", shell_quote(&recorder.to_string_lossy())));
    let out = quiet(Command::new("/bin/sh").arg("-c").arg(&line), &dir);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(&receipt).unwrap(), "game-arg\ntwo words\n");
}

/// The window mode with a scripted reading and the given DISPLAY (None: unset); (status, stdout, stderr, trace).
#[cfg(feature = "test-source")]
fn window_mode(display: Option<&str>) -> (i32, Vec<String>, Vec<String>, Vec<dehulud::json::Value>) {
    let mut o = Opts::new(Start::None).args(&[]).set("reading", "on foot|42|0.425||");
    if let Some(d) = display {
        o = o.env("DISPLAY", d);
    }
    let mut run = Run::start(o);
    run.wait_for("two polls", |p| int(p, "n") >= 2);
    let code = run.stop();
    (code, run.stdout(), run.stderr(), run.traced())
}

#[cfg(feature = "test-source")]
#[test]
fn display_elsewhere_falls_back_to_text_before_anything_is_connected() {
    for display in [Some("192.0.2.1:12"), Some("localhost:10"), Some("host/unix:0"), Some("tcp/:0"), Some(""), None] {
        let (code, out, err, trace) = window_mode(display);
        assert_eq!(code, 0, "{display:?}");
        assert!(out.iter().any(|l| l.contains(" 42% ")), "{display:?} {out:?}");
        assert_eq!(err.len(), 1, "one line saying why: {err:?}");
        assert!(err[0].contains("text mode"), "{err:?}");
        match display {
            Some(d) if !d.is_empty() => {
                assert!(err[0].contains(&format!("DISPLAY='{d}' is not a display on this computer")))
            }
            _ => assert!(err[0].contains("X11 or XWayland display"), "{err:?}"),
        }
        assert!(!trace.iter().any(|t| text(t, "event") == "connect"), "nothing was connected: {display:?}");
    }
}

#[cfg(feature = "test-source")]
#[test]
fn a_missing_local_display_is_tried_only_through_its_unix_sockets() {
    let (code, out, err, trace) = window_mode(Some(":987654"));
    assert_eq!(code, 0);
    assert!(out.iter().any(|l| l.contains(" 42% ")));
    assert_eq!(err.len(), 1, "{err:?}");
    assert!(err[0].starts_with("dehulud: no window possible (") && err[0].ends_with("), using text mode."), "{err:?}");
    let tried: Vec<String> = trace.iter().filter(|t| text(t, "event") == "connect").map(|t| text(t, "addr")).collect();
    assert_eq!(tried, ["/tmp/.X11-unix/X987654", "@/tmp/.X11-unix/X987654"]);
}

#[cfg(feature = "test-source")]
#[test]
fn text_mode_prints_each_detail_once_to_stderr() {
    let mut run = Run::start(
        Opts::new(Start::None)
            .set("reading", "waiting for game|-|-||")
            .set("reading", "no permission|-|-|right-click|FIX IT\\nsecond line"),
    );
    run.wait_for("a poll", |p| int(p, "n") >= 2);
    run.set(&[("step", "1")]);
    run.wait_for("the second reading", |p| text(p, "status") == "no permission");
    run.poll();
    run.stop();
    assert_eq!(run.stderr(), ["FIX IT", "second line"]);
    assert_eq!(
        run.stdout(),
        ["DeHulud   -  [....................] waiting for game", "DeHulud   -  [....................] no permission"]
    );
}
