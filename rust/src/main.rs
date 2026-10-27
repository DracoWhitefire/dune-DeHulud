//! DeHulud: read-only sandworm threat meter for Dune: Awakening on Linux. See `dehulud --help`.

fn main() {
    std::process::exit(dehulud::app::main(std::env::args_os().skip(1).collect()));
}
