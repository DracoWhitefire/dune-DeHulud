//! Test builds only (cargo feature `test-source`, never enabled for a release build): lets the integration
//! tests point the meter at THEIR fake game process instead of scanning /proc, and record every poll.
//!
//! A test build reads the control file named by the environment variable `DEHULUD_TEST_SOURCE` and refuses to
//! run without it, so a test can never scan the real /proc by accident. Without the feature none of this code
//! exists in the binary (see the no-op stand-ins in `lib.rs`).
//!
//! The control file holds `key=value` lines and is re-read on every use, so a test can change it while the
//! binary runs:
//!
//! | key | meaning |
//! |---|---|
//! | `pid=<n>` / `pid=child` | the only process listed (`child`: the `--launch` child); absent: none |
//! | `base=<n>` | image base of the fake game: its maps line is synthesized from it |
//! | `scope=0..3` / `scope=none` | ptrace_scope as the meter sees it (default 0) |
//! | `ppid=<n>` | parent pid reported for the listed process (the descendant check); absent: the real one |
//! | `gone=1` | the process is gone: not listed, its cmdline fails (a reaped process) |
//! | `deny=1` | every memory read fails with EPERM (permission tests) |
//! | `poll_ms=<n>`, `scan_ms=<n>` | poll interval and rescan interval |
//! | `stop=1` | end the meter (then exit, or wait for the --launch child) |
//! | `trace=<path>` | append one JSON line per poll and per window event |
//! | `reading=status\|percent\|fraction\|hint\|detail` | repeatable: a scripted meter instead of the real one |
//! | `step=<n>` | which scripted reading is shown |
//!
//! Only the listed process is ever looked at; every other pid is refused here.

use crate::json::quote;
use crate::meter::{Meter, Poll, Reading};
use crate::procfs::{self, Memory, Source};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;

const VAR: &str = "DEHULUD_TEST_SOURCE";

struct Control {
    keys: Vec<(String, String)>,
    readings: Vec<String>,
}

impl Control {
    fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    fn on(&self, key: &str) -> bool {
        self.get(key) == Some("1")
    }
    fn num(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|v| v.parse().ok())
    }
}

fn path() -> Option<PathBuf> {
    std::env::var_os(VAR).map(PathBuf::from)
}

fn control() -> Control {
    let text = path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let mut c = Control { keys: Vec::new(), readings: Vec::new() };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k == "reading" {
                c.readings.push(v.to_string());
            } else {
                c.keys.push((k.to_string(), v.to_string()));
            }
        }
    }
    c
}

/// A test build runs only with a control file.
pub fn require() {
    if path().is_none() {
        eprintln!("dehulud (test build): {VAR} must name the test's control file");
        std::process::exit(2);
    }
}

/// Appends one line to the trace file, if the test asked for one.
pub fn trace(line: impl FnOnce() -> String) {
    if let Some(p) = control().get("trace")
        && let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p)
    {
        let _ = writeln!(f, "{}", line());
    }
}

/// Records an attempt to reach the X display (the tests check that only Unix sockets are tried).
pub fn note_connect(addr: &str) {
    trace(|| format!("{{\"event\": \"connect\", \"addr\": {}}}", quote(addr)));
}

pub fn poll_interval() -> Option<Duration> {
    control().num("poll_ms").map(Duration::from_millis)
}

pub fn stop_requested() -> bool {
    control().on("stop")
}

/// The test source: one process (the test's own child) or none.
struct TestSource {
    child: Option<u32>,
}

impl TestSource {
    fn listed(&self, c: &Control) -> Option<i32> {
        if c.on("gone") {
            return None;
        }
        match c.get("pid") {
            Some("child") => self.child.map(|p| p as i32),
            Some(v) => v.parse().ok(),
            None => None,
        }
    }

    fn check(&self, pid: i32) -> io::Result<Control> {
        let c = control();
        if self.listed(&c) != Some(pid) {
            return Err(io::Error::from_raw_os_error(libc::ENOENT)); // never another process
        }
        Ok(c)
    }
}

impl Source for TestSource {
    fn pids(&self) -> Vec<i32> {
        self.listed(&control()).into_iter().collect()
    }
    fn uid(&self, pid: i32) -> io::Result<u32> {
        self.check(pid)?;
        Ok(fs::metadata(format!("/proc/{pid}"))?.uid())
    }
    fn cmdline(&self, pid: i32) -> io::Result<Vec<u8>> {
        self.check(pid)?;
        fs::read(format!("/proc/{pid}/cmdline"))
    }
    fn maps(&self, pid: i32) -> io::Result<String> {
        let c = self.check(pid)?;
        let Some(b) = c.num("base") else { return Ok(String::new()) };
        let size = c.num("size").unwrap_or(220008448); // the fake game's SizeOfImage
        Ok(format!(
            "55d000000000-55d000021000 r--p 00000000 103:02 77 /usr/bin/wine64-preloader\n\
             {b:x}-{:x} r--p 00000000 103:02 1234 /games/DUNE Awakening/DuneSandbox/Binaries/Win64/DuneSandbox-Win64-Shipping.exe\n\
             {:x}-{:x} rw-p 00000000 00:00 0 \n\
             7ffc00000000-7ffc00021000 rw-p 00000000 00:00 0 [stack]\n",
            b + 0x1000,
            b + 0x1000,
            b + size
        ))
    }
    fn ppid(&self, pid: i32) -> io::Result<i32> {
        let c = self.check(pid)?;
        if let Some(p) = c.get("ppid").and_then(|v| v.parse().ok()) {
            return Ok(p);
        }
        procfs::parse_ppid(&fs::read_to_string(format!("/proc/{pid}/status"))?)
    }
    fn ptrace_scope(&self) -> Option<i64> {
        match control().get("scope") {
            Some("none") => None,
            Some(v) => v.parse().ok(),
            None => Some(0),
        }
    }
}

/// Every read refused, as by the kernel without permission.
struct Denied(u64);

impl Memory for Denied {
    fn read(&mut self, _addr: u64, _size: usize) -> Result<Vec<u8>, i32> {
        self.0 += 1;
        Err(libc::EPERM)
    }
    fn count(&self) -> u64 {
        self.0
    }
}

fn reading_json(n: u64, r: &Reading, reads: u64, build: Option<&str>) -> String {
    let opt = |v: Option<String>| v.unwrap_or_else(|| "null".into());
    format!(
        "{{\"event\": \"poll\", \"n\": {n}, \"status\": {}, \"percent\": {}, \"fraction\": {}, \"hint\": {}, \"detail\": {}, \
         \"reads\": {reads}, \"build\": {}, \"line\": {}}}",
        quote(&r.status),
        opt(r.percent.map(|p| p.to_string())),
        opt(r.fraction.map(|f| format!("{f:?}"))),
        quote(&r.hint),
        quote(&r.detail),
        opt(build.map(quote)),
        quote(&crate::text::format_line(r))
    )
}

struct Traced {
    meter: Meter,
    n: u64,
}

impl Poll for Traced {
    fn poll(&mut self) -> Reading {
        let r = self.meter.poll();
        self.n += 1;
        trace(|| reading_json(self.n, &r, self.meter.reads, self.meter.attached_build()));
        r
    }
}

struct Scripted {
    n: u64,
}

fn parse_reading(s: &str) -> Reading {
    let f: Vec<&str> = s.splitn(5, '|').collect();
    let get = |i: usize| f.get(i).copied().unwrap_or("");
    Reading {
        status: get(0).to_string(),
        percent: get(1).parse().ok(),
        fraction: get(2).parse().ok(),
        hint: get(3).to_string(),
        detail: get(4).replace("\\n", "\n"),
    }
}

impl Poll for Scripted {
    fn poll(&mut self) -> Reading {
        let c = control();
        let step = c.num("step").unwrap_or(0) as usize;
        let r = c
            .readings
            .get(step.min(c.readings.len().saturating_sub(1)))
            .map_or(Reading::new("waiting for game"), |s| parse_reading(s));
        self.n += 1;
        trace(|| reading_json(self.n, &r, 0, None));
        r
    }
}

/// The meter of a test build.
pub fn meter(launched: bool, child: Option<u32>) -> Option<Box<dyn Poll>> {
    let c = control();
    if !c.readings.is_empty() {
        return Some(Box::new(Scripted { n: 0 }));
    }
    let deny = c.on("deny");
    let scan = Duration::from_millis(c.num("scan_ms").unwrap_or(2000));
    let meter = Meter::new(
        Box::new(TestSource { child }),
        Box::new(move |pid| -> Box<dyn Memory> {
            if deny { Box::new(Denied(0)) } else { Box::new(procfs::Reader::new(pid)) }
        }),
        launched,
        scan,
    );
    Some(Box::new(Traced { meter, n: 0 }))
}
