//! Everything DeHulud looks at outside the game's memory, and the read-only memory reader.
//!
//! [`ProcSource`] reads `/proc/<pid>/{cmdline,maps,status}` of the user's own processes (and stats
//! `/proc/<pid>` to learn the owner) plus `/proc/sys/kernel/yama/ptrace_scope`. [`Reader`] copies bytes out
//! of another process with `process_vm_readv`: one call per read, never a write.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;

/// What the meter needs to know about processes (tests use their own implementation).
pub trait Source {
    /// Process ids, sorted.
    fn pids(&self) -> Vec<i32>;
    /// Owner (real uid) of the process.
    fn uid(&self, pid: i32) -> io::Result<u32>;
    /// Raw `/proc/<pid>/cmdline`.
    fn cmdline(&self, pid: i32) -> io::Result<Vec<u8>>;
    /// `/proc/<pid>/maps` as text (invalid UTF-8 replaced).
    fn maps(&self, pid: i32) -> io::Result<String>;
    /// Parent process id from `/proc/<pid>/status` (0 when the line is missing).
    fn ppid(&self, pid: i32) -> io::Result<i32>;
    /// `kernel.yama.ptrace_scope`, or None when it cannot be read.
    fn ptrace_scope(&self) -> Option<i64>;
}

/// The real `/proc`.
pub struct ProcSource;

fn proc_file(pid: i32, name: &str) -> io::Result<Vec<u8>> {
    // name is one of cmdline, maps, status
    fs::read(format!("/proc/{pid}/{name}"))
}

/// The `PPid:` value of a `/proc/<pid>/status` text: Ok(0) without that line, an error when it is not a number.
pub fn parse_ppid(status: &str) -> io::Result<i32> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("PPid:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad PPid line"));
        }
    }
    Ok(0)
}

/// `kernel.yama.ptrace_scope` text to a value (surrounding whitespace allowed).
pub fn parse_scope(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

impl Source for ProcSource {
    fn pids(&self) -> Vec<i32> {
        let mut out: Vec<i32> = match fs::read_dir("/proc") {
            Ok(dir) => dir
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name();
                    let name = name.to_str()?;
                    if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) { name.parse().ok() } else { None }
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        out.sort_unstable();
        out
    }

    fn uid(&self, pid: i32) -> io::Result<u32> {
        Ok(fs::metadata(format!("/proc/{pid}"))?.uid())
    }

    fn cmdline(&self, pid: i32) -> io::Result<Vec<u8>> {
        proc_file(pid, "cmdline")
    }

    fn maps(&self, pid: i32) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&proc_file(pid, "maps")?).into_owned())
    }

    fn ppid(&self, pid: i32) -> io::Result<i32> {
        parse_ppid(&String::from_utf8_lossy(&proc_file(pid, "status")?))
    }

    fn ptrace_scope(&self) -> Option<i64> {
        parse_scope(&fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok()?)
    }
}

/// Read access to another process's memory (tests use their own implementation).
pub trait Memory {
    /// `size` bytes at `addr`, or the errno of the failed read.
    fn read(&mut self, addr: u64, size: usize) -> Result<Vec<u8>, i32>;
    /// Reads attempted so far (each counts once, failed or not; refused addresses do not count).
    fn count(&self) -> u64;
}

/// Copies bytes out of another process with `process_vm_readv`: one call per read, never a write.
pub struct Reader {
    pid: i32,
    count: u64,
}

impl Reader {
    pub fn new(pid: i32) -> Self {
        Reader { pid, count: 0 }
    }
}

/// True when `addr .. addr + size` lies in the lower half of the x86-64 address space (user space).
pub fn user_range(addr: u64, size: usize) -> bool {
    addr > 0 && u128::from(addr) < (1u128 << 47).saturating_sub(size as u128)
}

impl Memory for Reader {
    fn read(&mut self, addr: u64, size: usize) -> Result<Vec<u8>, i32> {
        if !user_range(addr, size) {
            return Err(libc::EFAULT); // address outside user space
        }
        self.count += 1;
        let mut buf = vec![0u8; size];
        let local = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: size };
        let remote = libc::iovec { iov_base: addr as usize as *mut libc::c_void, iov_len: size };
        // SAFETY: `local` describes `buf`, which is valid for `size` bytes and outlives the call; the remote
        // range is only read by the kernel, in the other process.
        let n = unsafe { libc::process_vm_readv(self.pid, &local, 1, &remote, 1, 0) };
        if n != size as isize {
            let code =
                if n < 0 { io::Error::last_os_error().raw_os_error().unwrap_or(libc::EFAULT) } else { libc::EFAULT };
            return Err(code);
        }
        Ok(buf)
    }

    fn count(&self) -> u64 {
        self.count
    }
}

/// This process's id and real uid.
pub fn me() -> (i32, u32) {
    // SAFETY: getuid has no preconditions and cannot fail.
    (std::process::id() as i32, unsafe { libc::getuid() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_errors() {
        let mut r = Reader::new(std::process::id() as i32);
        assert_eq!(r.read(8, 8), Err(libc::EFAULT)); // unmapped page 0 of our own process
        assert_eq!(r.count(), 1);
        assert_eq!(r.read(0, 8), Err(libc::EFAULT)); // refused before any call
        assert_eq!(r.read(1 << 47, 8), Err(libc::EFAULT));
        assert_eq!(r.read((1 << 47) - 8, 8), Err(libc::EFAULT)); // the end of the range is excluded
        assert_eq!(r.count(), 1);
    }

    #[test]
    fn reader_reads_own_memory() {
        let data: Box<[u8; 16]> = Box::new(*b"0123456789abcdef");
        let mut r = Reader::new(std::process::id() as i32);
        assert_eq!(r.read(data.as_ptr() as u64, 16).unwrap(), data.to_vec());
    }

    #[test]
    fn status_and_scope_parsing() {
        assert_eq!(parse_ppid("Name:\tx\nPPid:\t4321\nTracerPid:\t0\n").unwrap(), 4321);
        assert_eq!(parse_ppid("Name:\tx\n").unwrap(), 0);
        assert!(parse_ppid("PPid:\tx\n").is_err());
        assert_eq!(parse_scope("1\n"), Some(1));
        assert_eq!(parse_scope(" 3 "), Some(3));
        assert_eq!(parse_scope(""), None);
    }
}
