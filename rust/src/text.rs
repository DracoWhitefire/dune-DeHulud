//! The one-line terminal meter (`--text`, and the fallback without a local X display).

use crate::meter::{APP, MENU_HINT, POLL, Poll, Reading};
use crate::sections::Features;
use std::io::{self, Write};
use std::time::Duration;

/// One terminal line: name, percent (or '-'), bar, status.
pub fn format_line(r: &Reading) -> String {
    const WIDTH: usize = 20;
    let n = match r.percent {
        Some(_) => (r.fraction.unwrap_or(0.0) * WIDTH as f64) as usize,
        None => 0,
    };
    let pct = match r.percent {
        Some(p) => format!("{p:3}%"),
        None => "  - ".to_string(),
    };
    let tail = if !r.hint.is_empty() && r.hint != MENU_HINT && r.percent.is_none() {
        format!(" ({})", r.hint)
    } else {
        String::new()
    };
    format!("{APP} {pct} [{}{}] {}{tail}", "#".repeat(n), ".".repeat(WIDTH.saturating_sub(n)), r.status)
}

/// Text meter: redraws one line on a terminal, prints changed lines otherwise. Returns when `should_stop` says
/// so (the launched game ended, or Ctrl-C) or when the output is gone.
pub fn run_text(
    meter: &mut dyn Poll,
    out: &mut dyn Write,
    err: &mut dyn Write,
    tty: bool,
    should_stop: &mut dyn FnMut() -> bool,
    sleep: &mut dyn FnMut(Duration),
) -> io::Result<()> {
    let mut last: Option<String> = None;
    let mut said: Option<String> = None;
    while !should_stop() {
        let r = meter.poll_enabled(Features::TEXT); // the line is the worm meter's: only its route is read
        if !r.detail.is_empty() && said.as_deref() != Some(r.detail.as_str()) {
            let text = if tty { format!("\n{}\n", r.detail) } else { format!("{}\n", r.detail) };
            let _ = err.write_all(text.as_bytes());
            said = Some(r.detail.clone());
        }
        let line = format_line(&r);
        if tty {
            write!(out, "\r{line:<78}")?;
        } else if last.as_deref() != Some(line.as_str()) {
            writeln!(out, "{line}")?;
        }
        out.flush()?;
        last = Some(line);
        sleep(POLL);
    }
    if tty {
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format() {
        assert_eq!(format_line(&Reading::value("on foot", 42, 0.425)), "DeHulud  42% [########............] on foot");
        assert_eq!(
            format_line(&Reading::new("waiting for game")),
            "DeHulud   -  [....................] waiting for game"
        );
        assert_eq!(format_line(&Reading::value("vehicle", 100, 1.0)), "DeHulud 100% [####################] vehicle");
        assert_eq!(
            format_line(&Reading::with("no permission", "", MENU_HINT)),
            "DeHulud   -  [....................] no permission"
        );
        assert_eq!(
            format_line(&Reading::with("Unsupported game version", "x", "PE 1700000000")),
            "DeHulud   -  [....................] Unsupported game version (PE 1700000000)"
        );
        assert_eq!(format_line(&Reading::value("on foot", 0, 0.0)), "DeHulud   0% [....................] on foot");
    }

    struct Script(std::vec::IntoIter<Reading>);
    impl Poll for Script {
        fn poll_enabled(&mut self, enabled: Features) -> Reading {
            assert_eq!(enabled, Features::TEXT);
            self.0.next().expect("script")
        }
    }

    fn run(readings: Vec<Reading>, tty: bool) -> (String, String) {
        let n = readings.len();
        let mut m = Script(readings.into_iter());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut polls = 0;
        run_text(
            &mut m,
            &mut out,
            &mut err,
            tty,
            &mut || {
                polls += 1;
                polls > n
            },
            &mut |_| {},
        )
        .unwrap();
        (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    #[test]
    fn pipe_prints_changes_only() {
        let mut rs = vec![Reading::new("waiting for game"); 3];
        rs.extend(vec![Reading::value("on foot", 42, 0.425); 2]);
        rs.extend(vec![Reading::with("no permission", "FIX IT", "see help"); 2]);
        let (out, err) = run(rs, false);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains("42%"));
        assert_eq!(err.matches("FIX IT").count(), 1);
    }

    #[test]
    fn tty_redraws_one_line() {
        let (out, _) = run(vec![Reading::value("on foot", 42, 0.425); 2], true);
        assert_eq!(out.matches('\r').count(), 2);
        assert!(out.ends_with('\n'));
        assert_eq!(out.split('\r').nth(1).unwrap().len(), 78);
    }
}
