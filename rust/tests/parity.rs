//! Parity with the Python version's suite (tests/test_dehulud.py): the test build of the binary reads the
//! repository's fake game process (tests/fake_game.py) with process_vm_readv and must give the same statuses,
//! values and read counts for the same scenarios. Needs the `test-source` feature (see src/testhook.rs):
//!
//!     cargo test --release --features test-source
#![cfg(feature = "test-source")]

mod common;

use common::*;
use dehulud::help::{launch_option, shell_quote};
use dehulud::json::Value;

const TS: &str = "1792504044";

/// A fake game started beside the binary (a child of it, not a --launch child), attached.
fn game(fake_args: &[&str]) -> Run {
    let mut run = Run::start(Opts::new(Start::Beside).fake(fake_args));
    run.attach(&[]);
    run
}

fn on_foot_42(run: &Run) {
    let p = run.first_attached();
    assert_eq!(sp(&p), ("on foot".into(), Some(42)));
}

/// Applies the commands, then the next complete poll must show '-' (never 0).
fn dash(run: &mut Run, cmds: &[&str]) -> Value {
    for c in cmds {
        run.cmd(c);
    }
    let p = run.poll();
    assert_eq!(sp(&p).1, None, "{cmds:?} {p:?}");
    assert!(is_null(&p, "fraction"), "{cmds:?}");
    assert!(text(&p, "line").contains(" - "), "{cmds:?}");
    p
}

fn real_path() -> String {
    std::fs::canonicalize(bin()).unwrap().to_string_lossy().into_owned()
}

// ---- build identity -------------------------------------------------------------------------------------

#[test]
fn build_26083551_is_recognised_and_read_counts() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    let first = run.first_attached();
    assert_eq!(sp(&first), ("on foot".into(), Some(42)));
    assert_eq!(text(&first, "build"), "26083551");
    assert_eq!(int(&first, "reads"), 1 + 8 + 21); // header + 8 class slots + header start and the walk
    assert!((fraction(&first).unwrap() - 0.425).abs() < 1e-9);
    let next = run.poll();
    assert_eq!((sp(&next), int(&next, "reads")), (("on foot".into(), Some(42)), 21));
    assert_eq!(run.stop(), 0);
    let out = run.stdout();
    assert!(out.contains(&"DeHulud   -  [....................] waiting for game".to_string()), "{out:?}");
    assert!(out.contains(&"DeHulud  42% [########............] on foot".to_string()), "{out:?}");
    assert!(out.windows(2).all(|w| w[0] != w[1]), "a pipe gets changed lines only: {out:?}");
}

#[test]
fn build_26224975_is_recognised_and_routes() {
    if !readable() {
        return;
    }
    let mut run = game(&["--build", "26224975"]);
    let first = run.first_attached();
    assert_eq!(sp(&first), ("on foot".into(), Some(42)));
    assert_eq!(text(&first, "build"), "26224975");
    assert_eq!(int(&first, "reads"), 1 + 8 + 21); // header + 8 class slots + header start and the walk
    run.cmd("p pc 0x388 veh"); // AController.Pawn = the sandbike
    let p = run.wait_for("vehicle", |p| text(p, "status") == "vehicle");
    assert_eq!((sp(&p), int(&p, "reads")), (("vehicle".into(), Some(80)), 20));
    run.cmd("p pc 0x388 char");
    let p = run.wait_for("on foot", |p| text(p, "status") == "on foot");
    assert_eq!(sp(&p), ("on foot".into(), Some(42)));
    dash(&mut run, &["p pc 0x388 bogus"]);
    assert_eq!(run.stop(), 0);
}

#[test]
fn build_26333948_is_recognised_and_routes() {
    if !readable() {
        return;
    }
    let mut run = game(&["--build", "26333948"]);
    let first = run.first_attached();
    assert_eq!(sp(&first), ("on foot".into(), Some(42)));
    assert_eq!(text(&first, "build"), "26333948");
    assert_eq!(int(&first, "reads"), 1 + 8 + 21);
    run.cmd("p pc 0x388 veh"); // AController.Pawn
    let p = run.wait_for("vehicle", |p| text(p, "status") == "vehicle");
    assert_eq!((sp(&p), int(&p, "reads")), (("vehicle".into(), Some(80)), 20));
    run.cmd("p pc 0x388 char");
    let p = run.wait_for("on foot", |p| text(p, "status") == "on foot");
    assert_eq!(sp(&p), ("on foot".into(), Some(42)));
    dash(&mut run, &["p pc 0x388 bogus"]);
    assert_eq!(run.stop(), 0);
}

#[test]
fn unknown_build_is_unsupported_never_guessed() {
    if !readable() {
        return;
    }
    let mut run = game(&["--timestamp", "1700000000"]);
    let p = run.wait_for("Unsupported", |p| text(p, "status") == "Unsupported game version");
    assert_eq!(sp(&p).1, None);
    assert!(text(&p, "hint").contains("1700000000"));
    let detail = text(&p, "detail");
    assert!(detail.contains("1700000000") && detail.contains("26083551") && detail.contains("(2023-11-14 22:13 UTC)"));
    assert_eq!(int(&p, "reads"), 1); // only the PE header was read
    assert!(is_null(&p, "build"));
    let line = text(&p, "line");
    assert!(line.contains("Unsupported game version") && line.contains("1700000000"));
    assert_eq!(text(&run.poll(), "status"), "Unsupported game version");
    run.stop();
    assert!(
        run.stdout()
            .iter()
            .any(|l| l == "DeHulud   -  [....................] Unsupported game version (PE 1700000000)")
    );
    let said =
        run.stderr().iter().filter(|l| l.starts_with("Unsupported game version: PE timestamp 1700000000")).count();
    assert_eq!(said, 1, "the detail goes to stderr once");
}

// ---- image replaced under the same pid ------------------------------------------------------------------

#[test]
fn exec_into_an_unknown_image_is_unsupported_not_a_value() {
    if !readable() {
        return;
    }
    let mut run = game(&["--fixed"]);
    on_foot_42(&run);
    run.fake().exec_image(1700000000);
    let cmdline = std::fs::read(format!("/proc/{}/cmdline", run.fake().pid)).unwrap(); // our own grandchild
    assert!(String::from_utf8_lossy(&cmdline).contains("1700000000"));
    let p = run.poll();
    assert_eq!(sp(&p), ("Unsupported game version".into(), None));
    assert!(is_null(&p, "fraction") && is_null(&p, "build"));
    assert!(text(&p, "detail").contains("1700000000"));
    assert_eq!(text(&run.poll(), "status"), "Unsupported game version");
}

#[test]
fn exec_into_the_same_build_keeps_reading() {
    if !readable() {
        return;
    }
    let mut run = game(&["--fixed"]);
    on_foot_42(&run);
    run.fake().exec_image(TS.parse().unwrap());
    run.cmd("f char_comp 0x334 61");
    assert_eq!(sp(&run.poll()), ("on foot".into(), Some(61)));
    assert_eq!(int(&run.poll(), "reads"), 21); // the walk's 20 reads + the PE header start
}

// ---- the route ------------------------------------------------------------------------------------------

#[test]
fn vehicle_and_back_on_foot() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    run.cmd("p pc 0x388 veh"); // AController.Pawn = the sandbike
    let p = run.poll();
    assert_eq!((sp(&p), int(&p, "reads")), (("vehicle".into(), Some(80)), 20));
    run.cmd("f char_comp 0x334 10"); // the vehicle's component wins over the character's
    assert_eq!(sp(&run.poll()).1, Some(80));
    run.cmd("p pc 0x388 char"); // back on foot
    assert_eq!(sp(&run.poll()), ("on foot".into(), Some(10)));
}

#[test]
fn the_possessed_character_is_read() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    run.cmd("p pc 0x388 char2"); // Pawn == AcknowledgedPawn == the second character
    run.cmd("p pc 0x3f0 char2");
    assert_eq!(sp(&run.poll()), ("on foot".into(), Some(7)));
}

// Owner selection: the possessed Pawn decides. A vehicle when it passes the ADuneVehicle check; the character
// when it passes both character checks AND is the AcknowledgedPawn. Anything else shows '-', never the value of
// another object (in every case below, another object holds a valid 42 % or 7 %).
#[test]
fn owner_selection_never_falls_back_to_another_object() {
    if !readable() {
        return;
    }
    let cases: &[(&[&str], &str)] = &[
        (&["p pc 0x388 0"], "not in world"),                    // pawn null
        (&["p pc 0x388 veh", "p veh 0x10 c_bogus"], "no data"), // vehicle failing its class check
        (
            &[
                "p c_wheeled 0x40 c_bogus",
                "p c_bogus 0x40 c_world",
                "p c_world 0x40 c_lp",
                "p c_lp 0x40 c_veh",
                "p pc 0x388 veh",
            ],
            "no data",
        ), // vehicle deeper than the limit
        (&["p pc 0x3f0 char2"], "no data"),                     // another AcknowledgedPawn
        (&["p pc 0x388 lp"], "no data"),                        // non-character pawn
        (&["p pc 0x388 bogus"], "no data"),
    ];
    for (cmds, status) in cases {
        let mut run = game(&[]);
        on_foot_42(&run);
        assert_eq!(text(&dash(&mut run, cmds), "status"), *status, "{cmds:?}");
    }
}

#[test]
fn misaligned_pointer_is_rejected() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    assert_eq!(text(&dash(&mut run, &["i pc 0x388 12345"]), "status"), "no data");
}

#[test]
fn local_players_num_is_checked_before_data() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    run.cmd("i gi 0x40 0");
    let p = run.poll();
    assert_eq!((sp(&p), int(&p, "reads")), (("not in world".into(), None), 7)); // Data[0] is never read
    run.cmd("i gi 0x40 -1");
    assert_eq!(sp(&run.poll()).1, None);
    run.cmd("i gi 0x40 1");
    run.cmd("p gi 0x38 0");
    assert_eq!(text(&run.poll(), "status"), "not in world");
}

#[test]
fn missing_pawn_and_component() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    assert_eq!(text(&dash(&mut run, &["p pc 0x388 0", "p pc 0x3f0 0"]), "status"), "not in world");
    run.cmd("p pc 0x388 char");
    run.cmd("p pc 0x3f0 char");
    assert_eq!(sp(&run.poll()).1, Some(42));
    dash(&mut run, &["p char 0xe88 0"]);
}

#[test]
fn every_type_check() {
    if !readable() {
        return;
    }
    let cases: &[&[&str]] = &[
        &["p char 0xe88 bogus"],                  // threat component class
        &["p world 0x10 c_bogus"],                // UWorld
        &["p gi 0x10 c_bogus"],                   // UGameInstance
        &["p lp 0x10 c_bogus"],                   // ULocalPlayer
        &["p pc 0x10 c_lp"],                      // APlayerController
        &["p pc 0x388 lp", "p pc 0x3f0 lp"],      // pawn not a player character
        &["p char 0x10 c_char"],                  // ADuneCharacter but not ADunePlayerCharacter
        &["p pc 0x388 veh", "p veh 0xe08 bogus"], // vehicle component class
    ];
    for cmds in cases {
        let mut run = game(&[]);
        on_foot_42(&run);
        assert_eq!(text(&dash(&mut run, cmds), "status"), "no data", "{cmds:?}");
    }
}

#[test]
fn type_check_chain_depth_is_four() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    // BP_Bike(0) -> Wheeled(1) -> c_bogus(2) -> c_world(3) -> ADuneVehicle(4): accepted
    for c in ["p c_wheeled 0x40 c_bogus", "p c_bogus 0x40 c_world", "p c_world 0x40 c_veh", "p pc 0x388 veh"] {
        run.cmd(c);
    }
    assert_eq!(sp(&run.poll()).1, Some(80));
    // one more level: refused, while AcknowledgedPawn is still the character with a valid 42 %
    dash(&mut run, &["p c_world 0x40 c_lp", "p c_lp 0x40 c_veh"]);
}

#[test]
fn values_outside_the_domain_show_dash() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    for (blob, thr, why) in [
        ("nan", "100", "no value"),
        ("-1", "100", "no value"),
        ("inf", "100", "no value"),
        ("2e6", "100", "no value"),
        ("42", "0", "no threshold"),
        ("42", "-5", "no threshold"),
        ("42", "nan", "no threshold"),
        ("42", "inf", "no threshold"),
    ] {
        run.cmd(&format!("f char_comp 0x334 {blob}"));
        run.cmd(&format!("f char_comp 0x348 {thr}"));
        let p = run.poll();
        assert_eq!(sp(&p), (format!("on foot, {why}"), None), "{blob} {thr}");
        assert!(is_null(&p, "fraction"));
        assert!(!text(&p, "line").contains("0%"));
    }
}

#[test]
fn genuine_zero_and_floored_clamped_percent() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    run.cmd("f char_comp 0x334 0");
    let p = run.poll();
    assert_eq!((sp(&p).1, fraction(&p)), (Some(0), Some(0.0)));
    for (blob, want, frac) in [
        ("29", 29, 0.29),
        ("99.99", 99, 0.9999),
        ("100", 100, 1.0),
        ("150", 100, 1.0),
        ("1000", 100, 1.0),
        ("0.5", 0, 0.005),
    ] {
        run.cmd(&format!("f char_comp 0x334 {blob}"));
        let p = run.poll();
        assert_eq!(sp(&p).1, Some(want), "{blob}");
        assert!((fraction(&p).unwrap() - frac).abs() < 1e-5, "{blob}");
    }
}

#[test]
fn game_exit_between_polls() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    run.fake().exit(); // a zombie now: its pid cannot be reused
    let p = run.poll();
    assert_eq!(sp(&p), ("waiting for game".into(), None));
    assert!(is_null(&p, "build"));
    run.set(&[("gone", "1")]);
    assert_eq!(text(&run.poll(), "status"), "waiting for game");
}

#[test]
fn game_exit_and_reaped() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    run.set(&[("gone", "1")]); // as after reaping: the pid is no longer looked at
    run.fake().exit();
    let p = run.poll();
    assert_eq!(sp(&p), ("waiting for game".into(), None));
    assert!(is_null(&p, "build"));
}

#[test]
fn read_only() {
    if !readable() {
        return;
    }
    let mut run = game(&[]);
    on_foot_42(&run);
    let (comp, veh) = (run.fake().obj["char_comp"], run.fake().obj["veh_comp"]);
    let before = (run.fake().read_mem(comp, 0x400), run.fake().read_mem(veh, 0x400));
    for _ in 0..5 {
        run.poll();
    }
    run.cmd("p pc 0x388 veh");
    for _ in 0..5 {
        run.poll();
    }
    assert_eq!((run.fake().read_mem(comp, 0x400), run.fake().read_mem(veh, 0x400)), before);
}

// ---- permission -----------------------------------------------------------------------------------------

fn refused(scope: &str, launched: bool) -> Value {
    let mut run = if launched {
        Run::start(Opts::new(Start::Launch).set("deny", "1").set("scope", scope))
    } else {
        Run::start(Opts::new(Start::Beside).set("deny", "1").set("scope", scope))
    };
    if launched {
        let base = run.fake().base.to_string();
        run.set(&[("pid", "child"), ("base", &base)]);
    } else {
        run.attach(&[]);
    }
    let p = run.wait_for("no permission", |p| text(p, "status") == "no permission");
    assert_eq!(sp(&p).1, None);
    assert!(text(&p, "detail").contains("ptrace_scope"));
    assert!(!text(&p, "hint").is_empty());
    assert_eq!(text(&p, "line"), "DeHulud   -  [....................] no permission");
    p
}

#[test]
fn permission_help_per_scope() {
    let path = real_path();
    for (scope, launched, needles) in [
        ("0", false, vec!["same user".to_string()]),
        ("1", false, vec![launch_option(&path), "sudo sysctl kernel.yama.ptrace_scope=0".into()]),
        ("1", true, vec!["sudo sysctl kernel.yama.ptrace_scope=0".into(), "wrapper".into()]),
        ("2", false, vec!["sudo sysctl kernel.yama.ptrace_scope=1".into(), launch_option(&path)]),
        ("3", false, vec!["reboot".into(), "/etc/sysctl.d/".into()]),
        ("none", false, vec!["Yama".into(), "could not be read".into()]),
    ] {
        let detail = text(&refused(scope, launched), "detail");
        for n in needles {
            assert!(detail.contains(&n), "{scope} {launched}: {n}\n{detail}");
        }
    }
}

// ---- --launch -------------------------------------------------------------------------------------------

#[test]
fn launch_runs_the_command_and_exits_with_it() {
    let py = python();
    let mut run = Run::start(Opts::new(Start::None).args(&[
        "--text",
        "--launch",
        &py,
        "-c",
        "import time, sys; time.sleep(0.3); sys.exit(3)",
    ]));
    assert_eq!(run.wait(), 3);
    assert!(run.stdout().iter().any(|l| l.contains("waiting for game")));
    assert!(run.polls().len() >= 3, "the meter ran until the game ended");
}

#[test]
fn launch_of_a_missing_command() {
    let mut run = Run::start(Opts::new(Start::None).args(&["--text", "--launch", "/nonexistent/dune-game"]));
    assert_eq!(run.wait(), 127);
    assert!(
        run.stderr().iter().any(|l| l.starts_with("dehulud: cannot start /nonexistent/dune-game:")),
        "{:?}",
        run.stderr()
    );
}

#[test]
fn a_game_ended_by_a_signal_exits_with_128_plus_the_signal() {
    // the launched command ends itself with SIGTERM; DeHulud only reports it (it sends no signal)
    let mut run = Run::start(Opts::new(Start::None).args(&["--text", "--launch", "/bin/sh", "-c", "kill -TERM $$"]));
    assert_eq!(run.wait(), 128 + 15);
}

#[test]
fn launched_reads_its_descendant_at_every_scope() {
    if !readable() {
        return;
    }
    for scope in ["0", "1", "2", "3", "none"] {
        let mut run = Run::start(Opts::new(Start::Launch).set("scope", scope));
        let base = run.fake().base.to_string();
        run.set(&[("pid", "child"), ("base", &base)]);
        let p = run.first_attached();
        assert_eq!(sp(&p), ("on foot".into(), Some(42)), "{scope}");
        assert_eq!(text(&p, "detail"), "", "{scope}");
        run.fake().exit(); // the game ends: so does DeHulud, with its status
        assert_eq!(run.wait(), 0);
    }
}

/// --launch DeHulud (its child is `cat`), and a game that is not its descendant (the parent pid is reported as 1).
fn outside(scope: &str) -> Run {
    let mut run = Run::start(Opts::new(Start::Beside).args(&["--text", "--launch", "/bin/cat"]).set("scope", scope));
    run.attach(&[("ppid", "1")]);
    run
}

#[test]
fn outside_launch_with_scope_0_is_read_anyway() {
    if !readable() {
        return;
    }
    for scope in ["0", "none"] {
        let run = outside(scope);
        let p = run.first_attached();
        assert_eq!(sp(&p), ("on foot".into(), Some(42)), "{scope}");
        let pid = run.fake.as_ref().unwrap().pid;
        let detail = text(&p, "detail");
        assert!(
            detail.contains("found the game outside --launch; reading it anyway") && detail.contains(&pid.to_string())
        );
        assert_eq!(sp(&run.poll()).1, Some(42));
    }
}

fn refused_outside(scope: &str) -> String {
    let run = outside(scope);
    let p = run.wait_for("refused", |p| text(p, "status") == "not started by --launch");
    assert_eq!(sp(&p).1, None);
    assert_eq!(int(&p, "reads"), 0); // nothing was read from it
    let detail = text(&p, "detail");
    assert!(detail.contains(&run.fake.as_ref().unwrap().pid.to_string()));
    assert!(detail.contains("Steam started the game outside DeHulud"));
    assert!(!detail.contains("%command%")); // the launch option is what does not work here
    detail
}

#[test]
fn outside_launch_with_scopes_1_to_3() {
    let one = refused_outside("1");
    assert!(one.contains("sudo sysctl kernel.yama.ptrace_scope=0") && one.contains("remove the launch option"));
    assert!(one.contains(&shell_quote(&real_path())) && one.contains("Trade-off"));
    let two = refused_outside("2");
    assert!(
        two.contains("CAP_SYS_PTRACE")
            && two.contains("sudo sysctl kernel.yama.ptrace_scope=0")
            && two.contains("Trade-off")
    );
    let three = refused_outside("3");
    assert!(three.contains("reboot") && three.contains("cannot be lowered"));
    assert!(!three.contains("sudo sysctl kernel.yama.ptrace_scope="));
}

#[test]
fn lowering_the_scope_to_0_lets_the_running_meter_read_it() {
    if !readable() {
        return;
    }
    let mut run = outside("1");
    run.wait_for("refused", |p| text(p, "status") == "not started by --launch");
    run.set(&[("scope", "0")]);
    run.wait_for("42", |p| sp(p).1 == Some(42));
    run.close_stdin(); // `cat` ends: so does DeHulud
    assert_eq!(run.wait(), 0);
}
