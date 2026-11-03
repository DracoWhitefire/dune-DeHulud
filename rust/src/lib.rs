//! DeHulud - sandworm threat meter for Dune: Awakening on Linux (Rust version).
//!
//! The same meter as `dehulud.py` in the repository root, as one static binary: it finds the running
//! game, identifies its build from the PE header, reads the threat value and its threshold with
//! `process_vm_readv` (never a write, never a debugger attach, never a signal) and shows the result in a
//! small always-on-top X11 window or as one terminal line.
//!
//! Modules, in the order a poll uses them:
//! - [`procfs`]: what is read outside the game's memory (`/proc/<pid>/{cmdline,maps,status}`, the ptrace
//!   setting) and the `process_vm_readv` reader.
//! - [`meter`]: discovery, build identity, the route with its type checks, owner selection, value rules.
//! - [`help`]: the permission help texts and the quoted launch line.
//! - [`text`]: the one-line terminal meter.
//! - [`config`], [`json`]: the shared window config file.
//! - [`display`], [`font`], [`window`]: the X11 window (local Unix socket only, built-in bitmap font).
//! - [`app`]: command line, `--launch`, choosing window or text mode.
//!
//! The same crate builds a second program, DeHulud Combat (`dehulud-combat`): the worm meter plus damage taken
//! and damage dealt, sharing everything above. Its own parts:
//! - [`damage`]: the damage readers (health, shield and target tracking), called only by DeHulud Combat.
//! - [`sections`]: its window's switchable sections and the damage numbers they show.
//! - [`combat_config`], [`combat_window`], [`combat_app`]: its config file, window and command line.

pub mod app;
pub mod combat_app;
pub mod combat_config;
pub mod combat_window;
pub mod config;
pub mod damage;
pub mod display;
pub mod font;
pub mod help;
pub mod json;
pub mod meter;
pub mod procfs;
pub mod sections;
pub mod text;
pub mod window;

#[cfg(feature = "test-source")]
pub mod testhook;

/// Test builds only (feature `test-source`, see `testhook.rs`): no-op stand-ins in every other build, so a
/// normal build has no way to change what the meter reads.
#[cfg(not(feature = "test-source"))]
pub mod testhook {
    pub fn now() -> std::time::Instant {
        std::time::Instant::now()
    }
    pub fn route(_name: &'static str) {}
    pub fn trace(_line: impl FnOnce() -> String) {}
    pub fn note_connect(_addr: &str) {}
    pub fn poll_interval() -> Option<std::time::Duration> {
        None
    }
    pub fn stop_requested() -> bool {
        false
    }
}
