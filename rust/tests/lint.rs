//! Source lint: a check of the product source TEXT (src/), not a sandbox and not complete enforcement. It
//! fails when the code calls a memory-write, debugger, signal-sending, process-kill, network or
//! input-automation function, or names a network type. It cannot see what the libraries (std, libc, x11rb) do
//! at run time; how the window reaches its display is covered by the display tests (Unix sockets only).

use std::fs;
use std::path::Path;

/// Identifiers that must not appear in the code (comments are ignored).
const NAMES: &[&str] = &[
    // memory writes, debugger, other processes' private files
    "process_vm_writev",
    "ptrace",
    "PTRACE_ATTACH",
    "PTRACE_SEIZE",
    "PTRACE_POKEDATA",
    "PR_SET_PTRACER",
    "pwrite",
    "pwritev",
    "process_madvise",
    "fork",
    "vfork",
    "execv",
    "execve",
    "execvp",
    "system",
    "popen",
    "dlopen",
    // signals
    "kill",
    "killpg",
    "tgkill",
    "tkill",
    "pidfd_send_signal",
    "sigqueue",
    "raise",
    "pthread_kill",
    "send_signal",
    // network
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "ToSocketAddrs",
    "to_socket_addrs",
    "SocketAddrV4",
    "SocketAddrV6",
    "IpAddr",
    "Ipv4Addr",
    "Ipv6Addr",
    "AF_INET",
    "AF_INET6",
    "PF_INET",
    "sockaddr_in",
    "sockaddr_in6",
    "from_tcp_stream",
    "ConnectAddress",
    "parse_display",
    "getaddrinfo",
    // password / group database (may ask a local name service over a socket)
    "getpwuid",
    "getpwuid_r",
    "getpwnam",
    "getpwnam_r",
    "getpwent",
    "getgrgid",
    "getgrnam",
    // input automation
    "xtest",
    "XTest",
    "fake_input",
    "XTestFakeKeyEvent",
    "warp_pointer",
    "set_input_focus",
    "grab_keyboard",
];

/// Code fragments that must not appear (comments and string literals are ignored).
const FRAGMENTS: &[&str] = &[
    "std::net",
    "libc::socket",
    "libc::connect",
    "x11rb::connect",
    "RustConnection::connect(",
    "DefaultStream::connect",
];

/// Patterns that must not appear inside string literals.
const STRINGS: &[&str] =
    &["process_vm_writev", "/mem", "pagemap", "/environ", "ptrace(", "http://", "https://", "PTRACE_"];

/// The only libc functions the product calls.
const LIBC_CALLS: &[&str] =
    &["process_vm_readv", "getuid", "gethostname", "isatty", "sigaction", "sigemptyset", "poll"];

/// (code without comments and string contents, the string literals).
fn split(src: &str) -> (String, Vec<String>) {
    let b = src.as_bytes();
    let (mut code, mut strings) = (String::new(), Vec::new());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let mut depth = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if b[i] == b'r'
            && i + 1 < b.len()
            && (b[i + 1] == b'"' || b[i + 1] == b'#')
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
        {
            // raw string r"..." / r#"..."#
            let mut j = i + 1;
            let mut hashes = 0;
            while b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if b[j] != b'"' {
                code.push(b[i] as char);
                i += 1;
                continue;
            }
            let end = format!("\"{}", "#".repeat(hashes));
            let start = j + 1;
            let stop = src[start..].find(&end).map(|k| start + k).unwrap_or(b.len());
            strings.push(src[start..stop].to_string());
            code.push_str("\"\"");
            i = stop + end.len();
        } else if b[i] == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                if b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            strings.push(src[i + 1..j.min(b.len())].to_string());
            code.push_str("\"\"");
            i = j + 1;
        } else if b[i] == b'\'' && i + 2 < b.len() && (b[i + 2] == b'\'' || (b[i + 1] == b'\\' && i + 3 < b.len())) {
            // a char literal ('x', '\n', '\''); lifetimes ('a) are left alone
            let mut j = i + 1;
            if b[j] == b'\\' {
                j += 1;
            }
            j += 1;
            while j < b.len() && b[j] != b'\'' {
                j += 1;
            }
            code.push_str("' '");
            i = j + 1;
        } else {
            code.push(b[i] as char);
            i += 1;
        }
    }
    (code, strings)
}

fn identifiers(code: &str) -> Vec<&str> {
    code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect()
}

fn sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), fs::read_to_string(&p).unwrap()))
        .collect();
    out.sort();
    assert!(out.len() >= 10, "src/ not found");
    out
}

/// Each file's code with `#[cfg(test)]` unit-test modules removed (they are not part of the binary).
fn product(src: &str) -> String {
    match src.find("#[cfg(test)]\nmod tests").or_else(|| src.find("#[cfg(test)]\npub(crate) mod tests")) {
        Some(i) => src[..i].to_string(),
        None => src.to_string(),
    }
}

#[test]
fn forbidden_calls_absent() {
    let mut found = Vec::new();
    for (name, src) in sources() {
        let (code, strings) = split(&product(&src));
        for id in identifiers(&code) {
            if NAMES.contains(&id) {
                found.push(format!("{name}: {id}"));
            }
        }
        let flat: String = code.split_whitespace().collect();
        for frag in FRAGMENTS {
            // the fragment as a whole name: `x11rb::connect` is not `x11rb::connection`
            let hit = flat.match_indices(frag).any(|(i, _)| {
                frag.ends_with('(')
                    || !flat[i + frag.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            });
            if hit {
                found.push(format!("{name}: {frag}"));
            }
        }
        for s in &strings {
            for pat in STRINGS {
                if s.contains(pat) {
                    found.push(format!("{name}: string {:?}", &s[..s.len().min(40)]));
                }
            }
        }
    }
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn the_only_memory_call_is_process_vm_readv() {
    let all: String = sources().into_iter().map(|(_, s)| product(&s)).collect();
    let calls: Vec<&str> = all
        .match_indices("process_vm_")
        .map(|(i, _)| {
            let rest = &all[i..];
            &rest[..rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len())]
        })
        .collect();
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|c| *c == "process_vm_readv"), "{calls:?}");
}

#[test]
fn libc_calls_are_the_listed_ones() {
    let mut seen = Vec::new();
    for (name, src) in sources() {
        let (code, _) = split(&product(&src));
        let flat: String = code.split_whitespace().collect();
        for (i, _) in flat.match_indices("libc::") {
            let rest = &flat[i + 6..];
            let id = &rest[..rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len())];
            if rest[id.len()..].starts_with('(') && !LIBC_CALLS.contains(&id) {
                seen.push(format!("{name}: libc::{id}"));
            }
        }
    }
    assert_eq!(seen, Vec::<String>::new());
}

#[test]
fn one_event_is_sent_the_window_managers_keep_above_request() {
    // send_event appears once: the EWMH _NET_WM_STATE request to the root window (what Tk's -topmost sends)
    let mut sites = Vec::new();
    for (name, src) in sources() {
        let (code, _) = split(&product(&src));
        for (i, _) in code.match_indices("send_event") {
            let before = &code[..i];
            let func = before.rfind("fn ").map(|k| identifiers(&before[k + 3..])[0].to_string()).unwrap_or_default();
            sites.push(format!("{name}:{func}"));
        }
    }
    assert_eq!(sites, ["window.rs:keep_above"]);
}

#[test]
fn the_lint_sees_what_it_should() {
    // the lint itself: forbidden names in code are found, the same words in comments and strings are not
    let (code, strings) =
        split("let s = TcpStream::connect(a); // kill\n/* ptrace */ let t = \"kill \\\" https://x\"; let c = '\"';");
    let ids = identifiers(&code);
    assert!(ids.contains(&"TcpStream") && !ids.contains(&"kill") && !ids.contains(&"ptrace"));
    assert_eq!(strings, ["kill \\\" https://x"]);
}
