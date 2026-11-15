use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use flate2::Compression;
use flate2::write::GzEncoder;

use super::*;
use crate::store::testing::TempDataDir;
use crate::store::{OpenOptions, SiteSettings};

const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
const GOOGLEBOT: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";

/// One combined-format line on 14 Nov 2026 at `hour:minute:00 +0000`.
fn line(hour: u32, minute: u32, path: &str, referer: &str, ua: &str) -> String {
    format!(
        "203.0.113.{} - - [14/Nov/2026:{hour:02}:{minute:02}:00 +0000] \"GET {path} HTTP/2.0\" 200 1000 \"{referer}\" \"{ua}\"\n",
        minute % 200 + 1
    )
}

/// `count` lines, one a minute, starting at `hour:00`.
fn lines(hour: u32, count: u32) -> String {
    (0..count)
        .map(|i| line(hour + i / 60, i % 60, &format!("/page-{i}/"), "-", CHROME))
        .collect()
}

struct Fixture {
    tmp: TempDataDir,
    store: Store,
}

impl Fixture {
    fn new(name: &str, settings: SiteSettings) -> Self {
        let tmp = TempDataDir::new(name);
        let store = Store::create(&tmp.0, "site", &settings, &OpenOptions::default()).unwrap();
        Fixture { tmp, store }
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.tmp.0.root().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn ingest_with(&mut self, path: &Path, opts: &IngestOptions) -> Result<FileReport> {
        let cancel = AtomicBool::new(false);
        ingest(
            &mut self.store,
            &[path.to_path_buf()],
            opts,
            &mut |_| {},
            &cancel,
        )
        .map(|mut reports| reports.remove(0))
    }

    fn ingest(&mut self, path: &Path) -> FileReport {
        self.ingest_with(path, &IngestOptions::default()).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.store.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn rows(&self) -> i64 {
        self.count("SELECT count(*) FROM requests")
    }
}

fn gz(data: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

#[test]
fn ingests_and_enriches() {
    let mut fx = Fixture::new(
        "enrich",
        SiteSettings {
            hosts: vec!["www.example.com".into()],
            ..SiteSettings::default()
        },
    );
    let content = [
        line(10, 0, "/", "-", CHROME),
        line(10, 1, "/about/?utm_source=news&utm_medium=email", "-", CHROME),
        line(10, 2, "/contact/", "https://www.example.com/", CHROME),
        line(10, 3, "/products/", "https://www.google.co.uk/", CHROME),
        line(10, 4, "/style.css", "https://www.example.com/", CHROME),
        line(10, 5, "/products/", "-", GOOGLEBOT),
        line(10, 6, "/from-ai/", "https://chatgpt.com/", CHROME),
        "203.0.113.9 - - [14/Nov/2026:10:07:00 +0000] \"\\x16\\x03\\x01\" 400 166 \"-\" \"-\"\n".to_owned(),
        "2001:db8:1:2:3:4:5:6 - - [14/Nov/2026:10:08:00 +0000] \"GET / HTTP/2.0\" 200 5 \"-\" \"curl/8.5.0\"\n".to_owned(),
    ]
    .concat();
    let path = fx.file("a.log", content.as_bytes());

    let report = fx.ingest(&path);
    assert_eq!(report.status, FileStatus::Ingested);
    assert_eq!(
        (report.rows_ok, report.rows_bad, report.rows_skipped),
        (9, 0, 0)
    );
    assert_eq!(report.format.as_deref(), Some("combined"));
    assert_eq!(fx.rows(), 9);

    let by_source = |source: &str| {
        fx.count(&format!(
            "SELECT count(*) FROM requests WHERE source_type = '{source}'"
        ))
    };
    assert_eq!(by_source("direct"), 4);
    assert_eq!(by_source("campaign"), 1);
    assert_eq!(by_source("internal"), 2);
    assert_eq!(by_source("search"), 1);
    assert_eq!(by_source("ai"), 1);

    // Pages: browser GETs for non-assets. Not the CSS, the bot, curl or the garbage.
    assert_eq!(fx.count("SELECT count(*) FROM requests WHERE is_page"), 5);
    assert_eq!(
        fx.count("SELECT count(*) FROM requests WHERE raw_request IS NOT NULL AND path IS NULL"),
        1
    );
    assert_eq!(
        fx.count("SELECT count(*) FROM requests WHERE ip_net = '2001:db8:1::/48'"),
        1
    );
    assert_eq!(
        fx.count("SELECT count(*) FROM requests WHERE ip_net = '203.0.113.0/24'"),
        8
    );
    assert_eq!(fx.count("SELECT count(*) FROM user_agents"), 4);
    assert_eq!(
        fx.count(
            "SELECT count(*) FROM user_agents WHERE category = 'bot' AND family = 'Googlebot'"
        ),
        1
    );

    assert_eq!(
        fx.count("SELECT requests FROM daily_totals WHERE day = DATE '2026-11-14'"),
        9
    );
    assert_eq!(fx.count("SELECT pages FROM daily_totals"), 5);
    assert_eq!(fx.count("SELECT human FROM daily_totals"), 6);
    assert_eq!(fx.count("SELECT bot FROM daily_totals"), 2);
    assert_eq!(fx.count("SELECT sum(hits) FROM daily_sources"), 9);
}

#[test]
fn same_file_is_a_no_op_and_gzip_copy_too() {
    let mut fx = Fixture::new("dedupe", SiteSettings::default());
    let content = lines(1, 50);
    let plain = fx.file("access.log.1", content.as_bytes());
    let zipped = fx.file("access.log.1.gz", &gz(content.as_bytes()));

    assert_eq!(fx.ingest(&plain).status, FileStatus::Ingested);
    assert_eq!(fx.ingest(&plain).status, FileStatus::Unchanged);
    let report = fx.ingest(&zipped);
    assert_eq!(report.status, FileStatus::Unchanged);
    assert!(report.gzip);
    assert_eq!(fx.rows(), 50);
    assert_eq!(fx.count("SELECT count(*) FROM ingested_files"), 1);
}

#[test]
fn grown_file_adds_only_new_lines() {
    let mut fx = Fixture::new("grown", SiteSettings::default());
    let path = fx.file("access.log", lines(1, 50).as_bytes());
    fx.ingest(&path);

    // 30 more lines, and a half-written last line that must wait.
    let grown = format!("{}{}", lines(1, 80), "203.0.113.5 - - [14/Nov/2026:05:00");
    std::fs::write(&path, &grown).unwrap();
    let report = fx.ingest(&path);
    assert_eq!(report.status, FileStatus::Resumed);
    assert_eq!(report.rows_ok, 30);
    assert_eq!(fx.rows(), 80);
    assert_eq!(fx.count("SELECT count(*) FROM ingested_files"), 1);
    assert_eq!(fx.count("SELECT rows_ok FROM ingested_files"), 80);
    assert_eq!(fx.count("SELECT requests FROM daily_totals"), 80);

    // The half line completes.
    let done = format!("{grown}:00 +0000] \"GET /late/ HTTP/2.0\" 200 1 \"-\" \"x\"\n");
    std::fs::write(&path, &done).unwrap();
    assert_eq!(fx.ingest(&path).rows_ok, 1);
    assert_eq!(fx.rows(), 81);
    assert_eq!(fx.ingest(&path).status, FileStatus::Unchanged);
}

#[test]
fn large_grown_file_resumes_past_the_fingerprint_window() {
    let mut fx = Fixture::new("grown-large", SiteSettings::default());
    // About 180 KB each, so the head and tail windows are distinct regions.
    let first = lines(0, 1_000);
    let path = fx.file("access.log", first.as_bytes());
    assert_eq!(fx.ingest(&path).rows_ok, 1_000);

    let more: String = (0..400)
        .map(|i| line(17 + i / 60, i % 60, &format!("/more-{i}/"), "-", CHROME))
        .collect();
    std::fs::write(&path, format!("{first}{more}")).unwrap();
    let report = fx.ingest(&path);
    assert_eq!((report.status, report.rows_ok), (FileStatus::Resumed, 400));
    assert_eq!(fx.rows(), 1_400);

    // Same start and length, different content just before the resume point: a different file.
    let altered = format!("{first}{}", more.replacen("/more-399/", "/more-xxx/", 1));
    let other = fx.file("other.log", altered.as_bytes());
    let allow = IngestOptions {
        overlap: Some(OverlapPolicy::Allow),
        ..Default::default()
    };
    let report = fx.ingest_with(&other, &allow).unwrap();
    assert_eq!(
        (report.status, report.rows_ok),
        (FileStatus::Ingested, 1_400)
    );
}

#[test]
fn overlap_policies() {
    let mut fx = Fixture::new("overlap", SiteSettings::default());
    // 01:00–01:49, then a different file covering 01:30–02:29.
    let a = fx.file("a.log", lines(1, 50).as_bytes());
    let b_content: String = (30..90)
        .map(|i| line(1 + i / 60, i % 60, &format!("/b-{i}/"), "-", CHROME))
        .collect();
    let b = fx.file("b.log", b_content.as_bytes());
    fx.ingest(&a);

    let err = fx.ingest_with(&b, &IngestOptions::default()).unwrap_err();
    assert_eq!(err.code(), "overlap");
    assert!(err.to_string().contains("a.log"), "{err}");
    assert_eq!(fx.rows(), 50, "a rejected file leaves no rows");
    assert_eq!(fx.count("SELECT count(*) FROM ingested_files"), 1);

    let skip = IngestOptions {
        overlap: Some(OverlapPolicy::Skip),
        ..Default::default()
    };
    let report = fx.ingest_with(&b, &skip).unwrap();
    assert_eq!((report.rows_ok, report.rows_skipped), (40, 20));
    assert_eq!(fx.rows(), 90);

    // A third file fully inside covered time, allowed through.
    let c = fx.file("c.log", line(1, 10, "/c/", "-", CHROME).as_bytes());
    let allow = IngestOptions {
        overlap: Some(OverlapPolicy::Allow),
        ..Default::default()
    };
    assert_eq!(fx.ingest_with(&c, &allow).unwrap().rows_ok, 1);
    assert_eq!(fx.rows(), 91);
}

#[test]
fn force_replaces_a_files_rows() {
    let mut fx = Fixture::new("force", SiteSettings::default());
    let path = fx.file("a.log", lines(1, 50).as_bytes());
    fx.ingest(&path);
    let force = IngestOptions {
        force: true,
        ..Default::default()
    };
    let report = fx.ingest_with(&path, &force).unwrap();
    assert_eq!((report.status, report.rows_ok), (FileStatus::Ingested, 50));
    assert_eq!(fx.rows(), 50);
    assert_eq!(fx.count("SELECT count(*) FROM ingested_files"), 1);
    assert_eq!(fx.count("SELECT requests FROM daily_totals"), 50);
}

#[test]
fn bad_lines_are_counted_and_wrong_format_aborts() {
    let mut fx = Fixture::new("bad", SiteSettings::default());
    let mostly_good = format!("{}not a log line\n\n{}", lines(1, 20), lines(2, 20));
    let path = fx.file("ok.log", mostly_good.as_bytes());
    let report = fx.ingest(&path);
    assert_eq!((report.rows_ok, report.rows_bad), (40, 1));
    assert_eq!(report.bad_samples.len(), 1);
    assert_eq!(report.bad_samples[0].line, "not a log line");

    let json = fx.file("app.json", "{\"level\":\"info\"}\n".repeat(50).as_bytes());
    assert_eq!(
        fx.ingest_with(&json, &IngestOptions::default())
            .unwrap_err()
            .code(),
        "unknown_format"
    );
    let forced = IngestOptions {
        format: Some("combined".into()),
        ..Default::default()
    };
    assert_eq!(
        fx.ingest_with(&json, &forced).unwrap_err().code(),
        "wrong_format"
    );
    assert_eq!(fx.rows(), 40);

    let empty = fx.file("empty.log", b"");
    assert_eq!(fx.ingest(&empty).status, FileStatus::Empty);
}

#[test]
fn local_days_follow_the_site_time_zone() {
    let mut fx = Fixture::new(
        "tz",
        SiteSettings {
            timezone: chrono_tz::Australia::Sydney,
            ..SiteSettings::default()
        },
    );
    // 12:30 and 13:30 UTC on 14 Nov are 23:30 on the 14th and 00:30 on the 15th in Sydney (+11).
    let content = format!(
        "{}{}",
        line(12, 30, "/a/", "-", CHROME),
        line(13, 30, "/b/", "-", CHROME)
    );
    let path = fx.file("a.log", content.as_bytes());
    fx.ingest(&path);
    assert_eq!(
        fx.count("SELECT requests FROM daily_totals WHERE day = DATE '2026-11-14'"),
        1
    );
    assert_eq!(
        fx.count("SELECT requests FROM daily_totals WHERE day = DATE '2026-11-15'"),
        1
    );
    assert_eq!(
        fx.count("SELECT count(*) FROM requests WHERE ts_local = ts + INTERVAL 11 HOUR"),
        2
    );
}

#[test]
fn retention_none_replaces_previous_data() {
    let mut fx = Fixture::new(
        "retention",
        SiteSettings {
            retention: Retention::None,
            ..SiteSettings::default()
        },
    );
    let a = fx.file("a.log", lines(1, 10).as_bytes());
    let b = fx.file("b.log", lines(5, 7).as_bytes());
    fx.ingest(&a);
    fx.ingest(&b);
    assert_eq!(fx.rows(), 7);
    assert_eq!(fx.count("SELECT sum(requests) FROM daily_totals"), 7);
}
