//! Command line, `--launch`, and choosing the window or the text meter.

use crate::config;
use crate::display::{LocalDisplay, plain_error};
use crate::help::{self, py_repr};
use crate::meter::{Meter, POLL, Poll};
use crate::text::run_text;
use crate::window::Window;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};

/// `dehulud --help`, and the window's Help / details while there is no message to show.
pub const USAGE: &str = "DeHulud - sandworm threat meter for Dune: Awakening on Linux.

Shows how close you are to the sandworm attack threshold, as a percent and a bar, in a tiny
always-on-top window (or one terminal line). It finds the running game (Proton) by itself.

  dehulud                    window (falls back to text without a local X display)
  dehulud --text             one-line meter in the terminal
  dehulud --launch %command% Steam launch options: starts the game as a child of DeHulud, so a
                             ptrace_scope of 1 (descendants only) allows the read

Read-only: it reads /proc/<pid>/{cmdline,maps,status} of your own processes, the game's memory with
process_vm_readv, the ptrace setting and its own config file. It never writes to the game, never
attaches a debugger, never signals or injects anything, sends no input and opens no network
connection (the window only opens a local X display, through its Unix socket).
One static binary: no Python, no toolkit, no other files needed.";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Args {
    Run {
        text: bool,
        launch: Option<Vec<OsString>>,
    },
    /// Print `out` to stdout and `err` to stderr, then exit with `code`.
    Exit {
        code: i32,
        out: String,
        err: String,
    },
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
                        "dehulud: --launch needs the game command; Steam launch options: {}\n",
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
        if a != "--text" {
            let shown = py_repr(&a.to_string_lossy());
            if cmd.is_none() {
                return Args::Exit {
                    code: 1,
                    out: String::new(),
                    err: format!("{err}dehulud: unknown option {shown} (try --help)\n"),
                };
            }
            err.push_str(&format!("dehulud: ignoring unknown option {shown}\n")); // never block the game
        }
    }
    if !err.is_empty() {
        eprint!("{err}");
    }
    Args::Run { text: own.iter().any(|a| a == "--text"), launch: cmd }
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
    if let Some(m) = crate::testhook::meter(launched, child) {
        return m;
    }
    let _ = child;
    Box::new(Meter::real(launched))
}

/// The program: returns the exit status.
pub fn main(argv: Vec<OsString>) -> i32 {
    #[cfg(feature = "test-source")]
    crate::testhook::require();
    let program = help::program();
    let (text, cmd) = match parse_args(&argv, &program) {
        Args::Run { text, launch } => (text, launch),
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
                eprintln!("dehulud: cannot start {}: {}", OsStr::to_string_lossy(&cmd[0]), plain_error(&e));
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
        run_meter(&mut *meter, text, &mut stop)
    };
    if let Err(e) = result {
        if child.is_none() {
            eprintln!("dehulud: {e}");
            return 1;
        }
        eprintln!("dehulud: the meter stopped ({e}); the game keeps running.");
    }
    match child {
        Some(mut c) => match c.wait() {
            Ok(status) => exit_code(status),
            Err(e) => {
                eprintln!("dehulud: cannot wait for the game: {}", plain_error(&e));
                1
            }
        },
        None => 0,
    }
}

/// The window when there is a local X display (decided before any connection is made), else the text meter.
fn run_meter(meter: &mut dyn Poll, text: bool, stop: &mut dyn FnMut() -> bool) -> Result<(), String> {
    if !text {
        let display = std::env::var_os("DISPLAY").map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        match LocalDisplay::parse(&display) {
            None if display.is_empty() => eprintln!(
                "dehulud: no X display (DISPLAY is empty or not set; the window needs an X11 or XWayland display), \
                 using text mode."
            ),
            None => eprintln!(
                "dehulud: DISPLAY={} is not a display on this computer (DeHulud opens no network connection), using \
                 text mode.",
                py_repr(&display)
            ),
            Some(d) => match Window::new(&d, meter, config::config_path(), USAGE) {
                Ok(mut w) => return w.run(stop),
                Err(e) => eprintln!("dehulud: no window possible ({e}), using text mode."),
            },
        }
    }
    let interval = crate::testhook::poll_interval().unwrap_or(POLL);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut err = io::stderr();
    run_text(meter, &mut out, &mut err, is_tty(), stop, &mut |_| std::thread::sleep(interval))
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
        assert_eq!(parse_args(&os(&[]), "/p"), Args::Run { text: false, launch: None });
        assert_eq!(parse_args(&os(&["--text"]), "/p"), Args::Run { text: true, launch: None });
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
        assert_eq!(parse_args(&os(&argv), "/p"), Args::Run { text: false, launch: Some(os(&cmd)) });
        argv.insert(0, "--text");
        assert_eq!(parse_args(&os(&argv), "/p"), Args::Run { text: true, launch: Some(os(&cmd)) });
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
        assert_eq!((code, err.as_str()), (1, "dehulud: unknown option '--bogus' (try --help)\n"));
        // never blocks the game launch
        assert_eq!(
            parse_args(&os(&["--bogus", "--launch", "x"]), "/p"),
            Args::Run { text: false, launch: Some(os(&["x"])) }
        );
        let Args::Exit { code, out, .. } = parse_args(&os(&["-h"]), "/p") else { panic!() };
        assert!(code == 0 && out.starts_with("DeHulud - sandworm threat meter"));
    }

    #[test]
    fn exit_status_mapping() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(15)), 128 + 15); // killed by SIGTERM
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 128 + 9);
    }
}
