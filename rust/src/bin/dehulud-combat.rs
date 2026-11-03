//! DeHulud Combat: the sandworm threat meter plus damage taken and damage dealt, read-only, for Dune: Awakening
//! on Linux. See `dehulud-combat --help`.

fn main() {
    std::process::exit(dehulud::combat_app::main(std::env::args_os().skip(1).collect()));
}
