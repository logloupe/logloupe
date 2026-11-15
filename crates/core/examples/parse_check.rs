//! Parses a plain-text log with the combined parser and reports what it found.
//! A quick check of the parser against real logs, and a rough parse-only
//! throughput figure.
//!
//! `cargo run --release -p loga-core --example parse_check -- <file>...`

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Instant;

use loga_core::format::{Request, combined::parse_combined};

fn main() -> std::io::Result<()> {
    for path in std::env::args().skip(1) {
        let mut reader = BufReader::with_capacity(1 << 20, File::open(&path)?);
        let mut buf = Vec::new();
        let (mut ok, mut bad, mut malformed, mut trailing, mut bytes) =
            (0u64, 0u64, 0u64, 0u64, 0u64);
        let (mut first_ts, mut last_ts) = (i64::MAX, i64::MIN);
        let mut shown = 0;
        let start = Instant::now();

        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            let line = String::from_utf8_lossy(&buf);
            if line.trim().is_empty() {
                continue;
            }
            match parse_combined(&line) {
                Ok(p) => {
                    ok += 1;
                    first_ts = first_ts.min(p.record.ts);
                    last_ts = last_ts.max(p.record.ts);
                    if matches!(p.record.request, Request::Malformed(_)) {
                        malformed += 1;
                    }
                    if !p.rest.is_empty() {
                        trailing += 1;
                    }
                }
                Err(e) => {
                    bad += 1;
                    if shown < 5 {
                        shown += 1;
                        let preview: String = line.trim_end().chars().take(200).collect();
                        eprintln!("  bad ({e}): {preview}");
                    }
                }
            }
        }

        let secs = start.elapsed().as_secs_f64();
        println!(
            "{path}: ok={ok} bad={bad} malformed_requests={malformed} with_trailing_fields={trailing} \
             ts={first_ts}..{last_ts} {:.1} MB in {:.3}s ({:.0} MB/s)",
            bytes as f64 / 1e6,
            secs,
            bytes as f64 / 1e6 / secs
        );
    }
    Ok(())
}
