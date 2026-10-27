//! The window's X display: only a display on this computer, reached through its Unix socket.
//!
//! Accepted `DISPLAY` values: `:N`, `:N.S`, `unix:N`, `unix:N.S` (and `unix/:N[.S]`). Everything with a host
//! part is refused (`192.0.2.1:12`, `localhost:10` as set by ssh X forwarding, `host/unix:0`), as are socket
//! paths and anything else: the window must never open a network connection, and DeHulud then runs as the
//! text meter with a one-line reason.
//!
//! DeHulud opens the socket itself and hands the connected stream to x11rb (`connect_to_stream`), so no
//! library code ever decides how to reach the display (x11rb's and Xlib's own connect functions fall back to
//! TCP). For display N it tries, in this order:
//!
//! 1. the socket file `/tmp/.X11-unix/XN`: the X server's listening socket on disk. It is what sandboxes and
//!    containers (Flatpak, Steam's runtime container) bind into the programs they run, so it works there too.
//! 2. the abstract socket `@/tmp/.X11-unix/XN` (Linux): the same server's second listening socket, which needs no
//!    file. It covers a program running with a private /tmp, where the socket file is not visible.
//!
//! Both are Unix domain sockets: local to this computer by construction. TCP is never tried.

use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use x11rb::reexports::x11rb_protocol::xauth::{Family, get_auth};
use x11rb::rust_connection::{DefaultStream, RustConnection};

pub const SOCKET_DIR: &str = "/tmp/.X11-unix";

/// A display on this computer: number N and screen S of `:N.S`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDisplay {
    pub number: u64,
    pub screen: u32,
}

impl LocalDisplay {
    /// The display named `name` when it is a display on this computer, else None.
    pub fn parse(name: &str) -> Option<LocalDisplay> {
        let rest = name.strip_prefix("unix/").or_else(|| name.strip_prefix("unix")).unwrap_or(name);
        let rest = rest.strip_prefix(':')?;
        let (num, screen) = match rest.split_once('.') {
            Some((n, s)) => (n, Some(s)),
            None => (rest, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(num) || screen.is_some_and(|s| !digits(s)) {
            return None;
        }
        Some(LocalDisplay { number: num.parse().ok()?, screen: screen.map_or(Some(0), |s| s.parse().ok())? })
    }
}

/// Where a connection went: a socket file or an abstract socket name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Addr {
    Path(PathBuf),
    Abstract(Vec<u8>),
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Addr::Path(p) => write!(f, "{}", p.display()),
            Addr::Abstract(name) => write!(f, "@{}", String::from_utf8_lossy(name)),
        }
    }
}

/// The two Unix sockets of display `d`, in the order they are tried.
pub fn socket_addrs(d: &LocalDisplay, dir: &Path) -> [Addr; 2] {
    let path = dir.join(format!("X{}", d.number));
    let name = path.as_os_str().as_encoded_bytes().to_vec();
    [Addr::Path(path), Addr::Abstract(name)]
}

/// Connects a Unix stream to display `d` (socket file first, then the abstract socket). Never TCP.
pub fn connect_stream(d: &LocalDisplay, dir: &Path) -> Result<(UnixStream, Addr), String> {
    let mut errors = Vec::new();
    for addr in socket_addrs(d, dir) {
        crate::testhook::note_connect(&addr.to_string());
        let res = match &addr {
            Addr::Path(p) => UnixStream::connect(p),
            Addr::Abstract(name) => SocketAddr::from_abstract_name(name).and_then(|a| UnixStream::connect_addr(&a)),
        };
        match res {
            Ok(s) => return Ok((s, addr)),
            Err(e) => errors.push(format!("{addr}: {}", plain_error(&e))),
        }
    }
    Err(format!("cannot reach the X display :{} ({})", d.number, errors.join("; ")))
}

/// An io::Error without Rust's " (os error N)" suffix.
pub fn plain_error(e: &io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// This computer's host name (the address of local entries in the Xauthority file).
fn hostname() -> Vec<u8> {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for its whole length; gethostname writes at most that many bytes.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return Vec::new();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    buf[..n].to_vec()
}

/// A connected X display, the screen to use, and where the connection went.
pub struct Connected {
    pub conn: RustConnection<DefaultStream>,
    pub screen: usize,
    pub addr: Addr,
}

/// Opens display `d` through its Unix socket, with the Xauthority cookie ($XAUTHORITY or ~/.Xauthority) for
/// this host and display number when there is one.
pub fn connect(d: &LocalDisplay) -> Result<Connected, String> {
    let (stream, addr) = connect_stream(d, Path::new(SOCKET_DIR))?;
    let (stream, _) = DefaultStream::from_unix_stream(stream).map_err(|e| plain_error(&e))?;
    let (name, data) = u16::try_from(d.number)
        .ok()
        .and_then(|n| get_auth(Family::LOCAL, &hostname(), n).ok().flatten())
        .unwrap_or_default();
    let screen = d.screen as usize;
    let conn = RustConnection::connect_to_stream_with_auth_info(stream, screen, name, data)
        .map_err(|e| format!("X display :{}.{} refused the connection: {e}", d.number, d.screen))?;
    Ok(Connected { conn, screen, addr })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    pub const LOCAL: [(&str, u64, u32); 7] = [
        (":0", 0, 0),
        (":1", 1, 0),
        (":12.1", 12, 1),
        ("unix:0", 0, 0),
        ("unix:3.2", 3, 2),
        ("unix/:2", 2, 0),
        ("unix/:2.0", 2, 0),
    ];
    pub const REMOTE: [&str; 21] = [
        "192.0.2.1:12",
        "localhost:10",
        "localhost:0",
        "host:0",
        "[::1]:0",
        "::1:0",
        "tcp/:0",
        "inet/:0",
        "host/unix:0",
        "unix/host:0",
        "/tmp/.X11-unix/X0",
        ":",
        ":x",
        ":0.",
        ":0.0.0",
        " :0",
        ":0 ",
        ":0\n",
        "wayland-0",
        "",
        "unix",
    ];

    #[test]
    fn local_display_names() {
        for (name, number, screen) in LOCAL {
            assert_eq!(LocalDisplay::parse(name), Some(LocalDisplay { number, screen }), "{name}");
        }
        for name in REMOTE {
            assert_eq!(LocalDisplay::parse(name), None, "{name:?}");
        }
        assert_eq!(LocalDisplay::parse(":007"), Some(LocalDisplay { number: 7, screen: 0 }));
        assert_eq!(LocalDisplay::parse(":99999999999"), Some(LocalDisplay { number: 99999999999, screen: 0 }));
        assert_eq!(LocalDisplay::parse(":99999999999999999999999"), None); // beyond any display number
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dehulud-sock-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn socket_file_is_tried_first() {
        let dir = tmp("file");
        let _listener = UnixListener::bind(dir.join("X7")).unwrap();
        let d = LocalDisplay { number: 7, screen: 1 };
        let (_, addr) = connect_stream(&d, &dir).unwrap();
        assert_eq!(addr, Addr::Path(dir.join("X7")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn abstract_socket_is_the_fallback() {
        let dir = tmp("abstract"); // no socket file in it
        let name = dir.join("X8");
        let listener =
            UnixListener::bind_addr(&SocketAddr::from_abstract_name(name.as_os_str().as_encoded_bytes()).unwrap())
                .unwrap();
        let d = LocalDisplay { number: 8, screen: 0 };
        let (_, addr) = connect_stream(&d, &dir).unwrap();
        assert_eq!(addr.to_string(), format!("@{}", name.display()));
        drop(listener);
        let err = match connect_stream(&d, &dir) {
            Err(e) => e,
            Ok(_) => panic!("connected without a listener"),
        };
        assert!(
            err.contains(&format!("{}: ", name.display())) && err.contains(&format!("@{}: ", name.display())),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn addresses_are_unix_sockets_of_display_n() {
        let d = LocalDisplay { number: 5, screen: 0 };
        let [a, b] = socket_addrs(&d, Path::new(SOCKET_DIR));
        assert_eq!(a.to_string(), "/tmp/.X11-unix/X5");
        assert_eq!(b.to_string(), "@/tmp/.X11-unix/X5");
    }
}
