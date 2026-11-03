//! DeHulud Combat's command line, `--launch`, and choosing its window or the text meter (DeHulud's is
//! [`crate::app`]).

use crate::combat_config as config;
use crate::combat_window::Window;
use crate::display::{LocalDisplay, plain_error};
use crate::help::{self, py_repr};
use crate::meter::{Meter, POLL, Poll};
use crate::sections::{Feature, Features};
use crate::text::run_text;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};

/// `dehulud-combat --help`, and the window's Help / details while there is no message to show.
pub const USAGE: &str =
    "DeHulud Combat - sandworm threat meter, damage taken and damage dealt, for Dune: Awakening on Linux.

Shows how close you are to the sandworm attack threshold (a percent and a bar), the damage you take
and the damage you deal, in a small always-on-top window with a switch for each of the three
sections (or the worm meter as one terminal line). It finds the running game (Proton) by itself.

  dehulud-combat                    window (falls back to text without a local X display)
  dehulud-combat --text             one-line worm meter in the terminal
  dehulud-combat --launch %command% Steam launch options: starts the game as a child of DeHulud
                                    Combat, so a ptrace_scope of 1 (descendants only) allows the read
  --no-worm, --no-taken, --no-dealt
                                    start with that section of the window switched off, for this run
                                    only (a click on its label in the window's header row switches it
                                    and is saved); text mode shows only the worm meter, so --text
                                    refuses --no-worm

Read-only: it reads /proc/<pid>/{cmdline,maps,status} of your own processes, the game's memory with
process_vm_readv, the ptrace setting and its own config file. It never writes to the game, never
attaches a debugger, never signals or injects anything, sends no input and opens no network
connection (the window only opens a local X display, through its Unix socket).
One static binary: no Python, no toolkit, no other files needed.";

/// `--text` with `--no-worm`: the terminal line is the worm meter's, and it is switched off.
pub const TEXT_NO_WORM: &str = "dehulud-combat: text mode shows only the worm meter; drop --no-worm";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Args {
    Run {
        text: bool,
        launch: Option<Vec<OsString>>,
        /// Sections switched off by a flag, for this run only.
        off: Features,
    },
    /// Print `out` to stdout and `err` to stderr, then exit with `code`.
    Exit { code: i32, out: String, err: String },
}

/// Everything after `--launch` is the game's command, untouched.
pub fn parse_args(argv: &[OsString], program: &str) -> Args {
    let (own, cmd) = match argv.iter().position(|a| a == "--launch") {
        Some(i) => {
            let cmd = argv[i + 1..].to_vec();
            if cmd.is_empty() {
                return Args::Exit {
                    code: 1,
                    out: String::new(),
                    err: format!(
                        "dehulud-combat: --launch needs the game command; Steam launch options: {}\n",
                        help::launch_option(program)
                    ),
                };
            }
            (&argv[..i], Some(cmd))
        }
        None => (argv, None),
    };
    let mut err = String::new();
    for a in own {
        if a == "-h" || a == "--help" {
            return Args::Exit { code: 0, out: format!("{USAGE}\n"), err };
        }
        if a != "--text" && !Feature::ALL.iter().any(|f| a == f.flag()) {
            let shown = py_repr(&a.to_string_lossy());
            if cmd.is_none() {
                return Args::Exit {
                    code: 1,
                    out: String::new(),
                    err: format!("{err}dehulud-combat: unknown option {shown} (try --help)\n"),
                };
            }
            err.push_str(&format!("dehulud-combat: ignoring unknown option {shown}\n")); // never block the game
        }
    }
    if !err.is_empty() {
        eprint!("{err}");
    }
    let mut off = Features::NONE;
    for f in Feature::ALL {
        off.set(f, own.iter().any(|a| a == f.flag()));
    }
    let text = own.iter().any(|a| a == "--text");
    if text && off.worm {
        // nothing to show, and the worm route must not be read
        if cmd.is_none() {
            return Args::Exit { code: 1, out: String::new(), err: format!("{TEXT_NO_WORM}\n") };
        }
        eprintln!("{TEXT_NO_WORM} (the game starts without the meter)"); // never block the game
    }
    Args::Run { text, launch: cmd, off }
}

/// A game ended by signal N is reported as exit status 128 + N, as a shell reports it.
pub fn exit_code(status: ExitStatus) -> i32 {
    match status.code() {
        Some(code) => code,
        None => 128 + status.signal().unwrap_or(0),
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Ctrl-C ends the meter (and, with --launch, DeHulud keeps waiting for the game), like the Python version.
fn catch_ctrl_c() {
    // SAFETY: a zeroed sigaction with an empty mask and an async-signal-safe handler (one atomic store).
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

fn is_tty() -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(1) == 1 }
}

fn make_meter(launched: bool, child: Option<u32>) -> Box<dyn Poll> {
    #[cfg(feature = "test-source")]
    if let Some(m) = crate::testhook::meter(launched, child, true) {
        return m;
    }
    let _ = child;
    Box::new(Meter::combat(launched))
}

/// The program: returns the exit status.
pub fn main(argv: Vec<OsString>) -> i32 {
    #[cfg(feature = "test-source")]
    crate::testhook::require();
    let program = help::program();
    let (text, cmd, off) = match parse_args(&argv, &program) {
        Args::Run { text, launch, off } => (text, launch, off),
        Args::Exit { code, out, err } => {
            print!("{out}");
            eprint!("{err}");
            let _ = io::stdout().flush();
            return code;
        }
    };
    let mut child: Option<Child> = None;
    if let Some(cmd) = &cmd {
        // the game first: the meter must never stand in its way
        match Command::new(&cmd[0]).args(&cmd[1..]).spawn() {
            Ok(c) => child = Some(c),
            Err(e) => {
                eprintln!("dehulud-combat: cannot start {}: {}", OsStr::to_string_lossy(&cmd[0]), plain_error(&e));
                return 127;
            }
        }
    }
    catch_ctrl_c();
    let mut meter = make_meter(child.is_some(), child.as_ref().map(Child::id));
    let result = {
        let mut stop = || {
            INTERRUPTED.load(Ordering::SeqCst)
                || crate::testhook::stop_requested()
                || child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))))
        };
        run_meter(&mut *meter, text, off, &mut stop)
    };
    if result == Ok(false) && child.is_none() {
        return 1; // no window, and the text line's worm meter is switched off
    }
    if let Err(e) = result {
        if child.is_none() {
            eprintln!("dehulud-combat: {e}");
            return 1;
        }
        eprintln!("dehulud-combat: the meter stopped ({e}); the game keeps running.");
    }
    match child {
        Some(mut c) => match c.wait() {
            Ok(status) => exit_code(status),
            Err(e) => {
                eprintln!("dehulud-combat: cannot wait for the game: {}", plain_error(&e));
                1
            }
        },
        None => 0,
    }
}

/// The window when there is a local X display (decided before any connection is made), else the text meter.
/// Ok(false): no meter ran (text mode with the worm meter switched off: nothing to show, nothing is read).
fn run_meter(meter: &mut dyn Poll, text: bool, off: Features, stop: &mut dyn FnMut() -> bool) -> Result<bool, String> {
    if !text {
        let display = std::env::var_os("DISPLAY").map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        match LocalDisplay::parse(&display) {
            None if display.is_empty() => eprintln!(
                "dehulud-combat: no X display (DISPLAY is empty or not set; the window needs an X11 or XWayland display), \
                 using text mode."
            ),
            None => eprintln!(
                "dehulud-combat: DISPLAY={} is not a display on this computer (DeHulud opens no network connection), using \
                 text mode.",
                py_repr(&display)
            ),
            Some(d) => match Window::new(&d, meter, config::config_path(), USAGE, off) {
                Ok(mut w) => return w.run(stop).map(|()| true),
                Err(e) => eprintln!("dehulud-combat: no window possible ({e}), using text mode."),
            },
        }
    }
    if off.worm {
        if !text {
            eprintln!("{TEXT_NO_WORM}");
        }
        return Ok(false);
    }
    let interval = crate::testhook::poll_interval().unwrap_or(POLL);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut err = io::stderr();
    run_text(meter, &mut out, &mut err, is_tty(), stop, &mut |_| std::thread::sleep(interval))
        .map(|()| true)
        .map_err(|e| format!("cannot write the meter line: {}", plain_error(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_plain() {
        assert_eq!(parse_args(&os(&[]), "/p"), Args::Run { text: false, launch: None, off: Features::NONE });
        assert_eq!(parse_args(&os(&["--text"]), "/p"), Args::Run { text: true, launch: None, off: Features::NONE });
    }

    #[test]
    fn parse_launch_keeps_the_command_verbatim() {
        let cmd = [
            "/steam/reaper",
            "SteamLaunch",
            "AppId=1172710",
            "--",
            "proton",
            "waitforexitandrun",
            "DuneSandbox.exe",
            "--text",
            "--launch",
        ];
        let mut argv = vec!["--launch"];
        argv.extend(cmd);
        assert_eq!(
            parse_args(&os(&argv), "/p"),
            Args::Run { text: false, launch: Some(os(&cmd)), off: Features::NONE }
        );
        argv.insert(0, "--text");
        assert_eq!(parse_args(&os(&argv), "/p"), Args::Run { text: true, launch: Some(os(&cmd)), off: Features::NONE });
    }

    #[test]
    fn parse_launch_without_command() {
        let Args::Exit { code, err, .. } = parse_args(&os(&["--launch"]), "/x y/dehulud") else { panic!() };
        assert_eq!(code, 1);
        assert!(err.contains("'/x y/dehulud' --launch %command%"), "{err}");
    }

    #[test]
    fn parse_unknown_option() {
        let Args::Exit { code, err, .. } = parse_args(&os(&["--bogus"]), "/p") else { panic!() };
        assert_eq!((code, err.as_str()), (1, "dehulud-combat: unknown option '--bogus' (try --help)\n"));
        // never blocks the game launch
        assert_eq!(
            parse_args(&os(&["--bogus", "--launch", "x"]), "/p"),
            Args::Run { text: false, launch: Some(os(&["x"])), off: Features::NONE }
        );
        let Args::Exit { code, out, .. } = parse_args(&os(&["-h"]), "/p") else { panic!() };
        assert!(code == 0 && out.starts_with("DeHulud Combat - sandworm threat meter"));
    }

    #[test]
    fn parse_section_flags() {
        let run = |argv: &[&str]| match parse_args(&os(argv), "/p") {
            Args::Run { text, launch, off } => (text, launch, off),
            other => panic!("{other:?}"),
        };
        let only = |worm, taken, dealt| Features { worm, taken, dealt };
        assert_eq!(run(&["--no-worm"]).2, only(true, false, false));
        assert_eq!(run(&["--no-taken"]).2, only(false, true, false));
        assert_eq!(run(&["--no-dealt"]).2, only(false, false, true));
        assert_eq!(run(&["--no-dealt", "--no-worm"]), (false, None, only(true, false, true)));
        // after --launch everything belongs to the game
        assert_eq!(
            run(&["--no-taken", "--launch", "game", "--no-worm"]),
            (false, Some(os(&["game", "--no-worm"])), only(false, true, false))
        );
        assert!(matches!(parse_args(&os(&["--no-wurm"]), "/p"), Args::Exit { code: 1, .. }));
        // text mode shows only the worm meter: --text --no-worm is a usage error; the damage flags change nothing
        for argv in [&["--text", "--no-worm"][..], &["--no-worm", "--no-dealt", "--text"]] {
            assert_eq!(
                parse_args(&os(argv), "/p"),
                Args::Exit { code: 1, out: String::new(), err: format!("{TEXT_NO_WORM}\n") },
                "{argv:?}"
            );
        }
        assert_eq!(run(&["--text", "--no-taken", "--no-dealt"]), (true, None, only(false, true, true)));
        // with --launch the game still starts (never blocked); no meter runs
        assert_eq!(run(&["--text", "--no-worm", "--launch", "g"]), (true, Some(os(&["g"])), only(true, false, false)));
        for f in Feature::ALL {
            assert!(USAGE.contains(f.flag()), "{}", f.flag());
        }
    }

    #[test]
    fn exit_status_mapping() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(15)), 128 + 15); // killed by SIGTERM
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 128 + 9);
    }
}
