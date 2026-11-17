use std::cell::{Cell, RefCell};

use super::*;
use crate::ingest::FileStatus;
use crate::maintain;
use crate::store::testing::TempDataDir;
use crate::store::{OpenOptions, SiteSettings};

const DAY: i64 = 86_400;

/// `count` combined-format lines, one a minute, starting at `hour:00` on 14 Nov 2026.
fn log_lines(hour: u32, count: u32) -> String {
    (0..count)
        .map(|i| {
            format!(
                "203.0.113.{} - - [14/Nov/2026:{:02}:{:02}:00 +0000] \"GET /page-{i}/ HTTP/2.0\" 200 1000 \"-\" \"Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0\"\n",
                i % 200 + 1,
                hour + i / 60,
                i % 60
            )
        })
        .collect()
}

fn remote() -> Remote {
    Remote {
        host: "web1".into(),
        port: None,
        paths: vec!["/var/log/nginx/access.log*".into()],
    }
}

/// A server that exists only in memory.
#[derive(Default)]
struct Fake {
    /// Path to content and modification time.
    files: RefCell<BTreeMap<String, (Vec<u8>, i64)>>,
    /// Send this many bytes fewer than asked for.
    short_by: Cell<u64>,
    downloads: Cell<usize>,
}

impl Fake {
    fn put(&self, path: &str, content: &str, mtime: i64) {
        self.files
            .borrow_mut()
            .insert(path.to_owned(), (content.as_bytes().to_vec(), mtime));
    }
}

impl Transport for Fake {
    fn list(&self, _remote: &Remote) -> Result<String> {
        let mut out = format!("Welcome to the server\n{LIST_BEGIN}\nP 0\n");
        for (path, (content, mtime)) in self.files.borrow().iter() {
            out.push_str(&format!("F {} {mtime} {path}\n", content.len()));
        }
        out.push_str(LIST_END);
        out.push('\n');
        Ok(out)
    }

    fn download(
        &self,
        _remote: &Remote,
        path: &str,
        size: u64,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
        _cancel: &AtomicBool,
    ) -> Result<u64> {
        self.downloads.set(self.downloads.get() + 1);
        let files = self.files.borrow();
        let content = &files.get(path).expect("only listed files are fetched").0;
        let take = (size - self.short_by.get()).min(content.len() as u64) as usize;
        dest.write_all(&content[..take])?;
        progress(take as u64);
        Ok(take as u64)
    }
}

struct Fixture {
    tmp: TempDataDir,
    store: Store,
    server: Fake,
}

impl Fixture {
    fn new(name: &str) -> Self {
        Self::with_settings(name, SiteSettings::default())
    }

    fn with_settings(name: &str, settings: SiteSettings) -> Self {
        let tmp = TempDataDir::new(name);
        let store = Store::create(&tmp.0, "site", &settings, &OpenOptions::default()).unwrap();
        set(&store, &remote()).unwrap();
        Fixture {
            tmp,
            store,
            server: Fake::default(),
        }
    }

    fn fetch_with(&mut self, opts: &FetchOptions) -> Result<FetchReport> {
        fetch(
            &mut self.store,
            opts,
            &self.server,
            &mut |_| {},
            &AtomicBool::new(false),
        )
    }

    fn fetch(&mut self) -> FetchReport {
        self.fetch_with(&FetchOptions::default()).unwrap()
    }

    fn rows(&self) -> i64 {
        self.store
            .conn
            .query_row("SELECT count(*) FROM requests", [], |r| r.get(0))
            .unwrap()
    }

    fn states(&self) -> Vec<(String, FileState)> {
        list(&self.store, None, &self.server)
            .unwrap()
            .files
            .into_iter()
            .map(|f| (f.path, f.state))
            .collect()
    }

    fn leftovers(&self) -> usize {
        std::fs::read_dir(self.tmp.0.root().join("tmp"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("fetch-"))
            .count()
    }
}

#[test]
fn hosts_are_checked() {
    let with_host = |host: &str| Remote {
        host: host.into(),
        ..remote()
    };
    for ok in ["web1", "deploy@example.com", "192.0.2.10", "a_b-c.example"] {
        assert!(with_host(ok).validate().is_ok(), "{ok}");
    }
    for bad in [
        "",
        "-oProxyCommand=calc",
        "deploy@-oProxyCommand=calc",
        "web1 -v",
        "web1;id",
        "a@b@c",
        "@web1",
        "web1@",
        "[2001:db8::1]",
        "web1\n",
    ] {
        assert!(with_host(bad).validate().is_err(), "{bad:?}");
    }
    assert!(
        Remote {
            port: Some(0),
            ..remote()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn patterns_are_checked() {
    let with_path = |path: &str| Remote {
        paths: vec![path.into()],
        ..remote()
    };
    for ok in [
        "/var/log/nginx/access.log*",
        "~/logs/example.com-???-2026.gz",
        "/var/log/apache2/access.log.[1-7]",
        "/srv/logs/site_a/access+b,c=d@e%f:g.log",
    ] {
        assert!(with_path(ok).validate().is_ok(), "{ok}");
    }
    for bad in [
        "access.log",
        "~user/logs/*",
        "/var/log/~/x",
        "/var/log/$(id)",
        "/var/log/`id`",
        "/var/log/a b",
        "/var/log/a;b",
        "/var/log/a|b",
        "/var/log/a&b",
        "/var/log/a>b",
        "/var/log/'a'",
        "/var/log/\"a\"",
        "/var/log/a\\b",
        "/var/log/{a,b}",
        "/var/log/a!b",
        "/var/log/a\nrm",
        "/var/log/a#b",
    ] {
        assert!(with_path(bad).validate().is_err(), "{bad:?}");
    }
    assert!(
        Remote {
            paths: Vec::new(),
            ..remote()
        }
        .validate()
        .is_err()
    );
    assert!(
        Remote {
            paths: vec!["/a".into(); MAX_PATTERNS + 1],
            ..remote()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn scripts_quote_what_they_are_given() {
    let script = list_script(&remote());
    assert!(script.contains("for f in /var/log/nginx/access.log*; do"));
    assert!(script.contains(LIST_BEGIN) && script.trim_end().ends_with(LIST_END));

    assert_eq!(
        download_script("/var/log/nginx/access.log.1", 42).unwrap(),
        "head -c 42 -- '/var/log/nginx/access.log.1'\n"
    );
    for bad in [
        "/var/log/it's.log",
        "/var/log/a b.log",
        "relative.log",
        "/var/log/$(id).log",
        "/var/log/a*.log",
        "/var/log/a\nb",
    ] {
        assert!(download_script(bad, 1).is_err(), "{bad:?}");
    }
}

#[test]
fn listing_output_is_read_defensively() {
    let remote = Remote {
        paths: vec!["/var/log/a*".into(), "/var/log/b*".into()],
        ..remote()
    };
    let output = format!(
        "motd text\nF 1 1 /etc/ignored-before-the-marker\n{LIST_BEGIN}\r\n\
         P 0\n\
         F 120 1700000000 /var/log/a.log\n\
         F 120 1700000000 /var/log/a.log\n\
         F 7 1700000100 /var/log/a b.log\n\
         F x y /var/log/a2.log\n\
         U /var/log/a-secret.log\n\
         something else\n\
         P 1\n\
         {LIST_END}\n\
         F 1 1 /etc/ignored-after-the-marker\n"
    );
    let (files, notes) = parse_listing(&output, &remote).unwrap();
    assert_eq!(
        files,
        vec![Listed {
            path: "/var/log/a.log".into(),
            size: 120,
            mtime: 1_700_000_000
        }]
    );
    let notes = notes.join("\n");
    assert!(notes.contains("Nothing on the server matches /var/log/b*"));
    assert!(!notes.contains("matches /var/log/a*"));
    assert!(notes.contains("1 file(s) cannot be read") && notes.contains("/var/log/a-secret.log"));
    assert!(notes.contains("1 file(s) were left out"));
    assert!(notes.contains("2 line(s)"));

    // No markers at all, and a listing that stops half way.
    assert!(matches!(
        parse_listing("sh: command not found\n", &remote),
        Err(Error::Remote { .. })
    ));
    assert!(matches!(
        parse_listing(&format!("{LIST_BEGIN}\nP 0\n"), &remote),
        Err(Error::Remote { .. })
    ));
}

#[test]
fn settings_round_trip() {
    let fx = Fixture::new("remote-settings");
    assert_eq!(get(&fx.store).unwrap(), Some(remote()));

    let stored = set(
        &fx.store,
        &Remote {
            host: " deploy@example.com ".into(),
            port: Some(2222),
            paths: vec![
                " /var/log/a* ".into(),
                String::new(),
                "/var/log/a*".into(),
                "~/logs/*.gz".into(),
            ],
        },
    )
    .unwrap();
    assert_eq!(stored.host, "deploy@example.com");
    assert_eq!(stored.paths, vec!["/var/log/a*", "~/logs/*.gz"]);
    assert_eq!(get(&fx.store).unwrap(), Some(stored));

    // A bad value is refused and the old one kept.
    assert!(
        set(
            &fx.store,
            &Remote {
                host: "-oProxyCommand=calc".into(),
                ..remote()
            }
        )
        .is_err()
    );
    assert_eq!(get(&fx.store).unwrap().unwrap().port, Some(2222));

    assert!(remove(&fx.store).unwrap());
    assert!(!remove(&fx.store).unwrap());
    assert_eq!(get(&fx.store).unwrap(), None);
    assert!(matches!(
        list(&fx.store, None, &fx.server),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn fetches_new_files_once() {
    let mut fx = Fixture::new("remote-fetch");
    let now = now_secs();
    fx.server
        .put("/var/log/nginx/access.log.1", &log_lines(8, 30), now - DAY);
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now);

    assert!(fx.states().iter().all(|(_, s)| *s == FileState::New));

    let mut seen_progress = Vec::new();
    let report = fetch(
        &mut fx.store,
        &FetchOptions::default(),
        &fx.server,
        &mut |p| seen_progress.push((p.phase, p.path.to_owned(), p.file, p.files)),
        &AtomicBool::new(false),
    )
    .unwrap();

    // Oldest first, named by their path on the server.
    let paths: Vec<String> = report
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(
        paths,
        ["/var/log/nginx/access.log.1", "/var/log/nginx/access.log"]
    );
    assert!(
        report
            .files
            .iter()
            .all(|f| f.status == FileStatus::Ingested)
    );
    assert_eq!(
        report.bytes_downloaded as usize,
        log_lines(8, 30).len() + log_lines(10, 20).len()
    );
    assert_eq!(fx.rows(), 50);
    assert!(seen_progress.contains(&(
        Phase::Download,
        "/var/log/nginx/access.log.1".to_owned(),
        1,
        2
    )));
    assert!(seen_progress.contains(&(Phase::Ingest, "/var/log/nginx/access.log".to_owned(), 2, 2)));

    let stored = maintain::files(&fx.store).unwrap();
    assert!(stored.iter().all(|f| f.source == "ssh:web1"));
    assert_eq!(stored[1].path, "/var/log/nginx/access.log");
    assert_eq!(fx.leftovers(), 0);

    // Nothing has changed: no download at all.
    let again = fx.fetch();
    assert!(again.files.is_empty());
    assert_eq!(again.already_fetched, 2);
    assert_eq!(fx.server.downloads.get(), 2);
    assert!(fx.states().iter().all(|(_, s)| *s == FileState::Fetched));
}

#[test]
fn grown_and_rotated_files() {
    let mut fx = Fixture::new("remote-grown");
    let now = now_secs();
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now - 60);
    fx.fetch();

    // The live log grows: only its new lines are added.
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 35), now - 30);
    assert_eq!(fx.states()[0].1, FileState::Changed);
    let report = fx.fetch();
    assert_eq!(report.files[0].status, FileStatus::Resumed);
    assert_eq!(report.files[0].rows_ok, 15);
    assert_eq!(fx.rows(), 35);

    // It is rotated and a new one started. The old content under its new
    // name adds nothing; the new log is ingested.
    let old = fx.server.files.borrow()["/var/log/nginx/access.log"].clone();
    fx.server
        .files
        .borrow_mut()
        .insert("/var/log/nginx/access.log.1".into(), old);
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(12, 10), now);
    let report = fx.fetch();
    let statuses: Vec<FileStatus> = report.files.iter().map(|f| f.status).collect();
    assert_eq!(statuses, [FileStatus::Unchanged, FileStatus::Ingested]);
    assert_eq!(fx.rows(), 45);

    // Both names are now remembered.
    assert!(fx.fetch().files.is_empty());
}

#[test]
fn deleted_files_are_fetched_again() {
    let mut fx = Fixture::new("remote-deleted");
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now_secs());
    let file_id = fx.fetch().files[0].file_id.unwrap();

    maintain::delete_file(&fx.store, file_id).unwrap();
    assert_eq!(fx.rows(), 0);
    assert_eq!(fx.states()[0].1, FileState::New);
    fx.fetch();
    assert_eq!(fx.rows(), 20);
}

#[test]
fn force_and_days() {
    let mut fx = Fixture::new("remote-options");
    let now = now_secs();
    fx.server.put(
        "/var/log/nginx/access.log.9",
        &log_lines(1, 10),
        now - 40 * DAY,
    );
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now);

    let recent = FetchOptions {
        days: Some(7),
        ..FetchOptions::default()
    };
    let report = fx.fetch_with(&recent).unwrap();
    assert_eq!((report.files.len(), report.too_old), (1, 1));
    assert_eq!(fx.rows(), 20);
    let listing = list(&fx.store, Some(7), &fx.server).unwrap();
    assert_eq!(listing.files[0].state, FileState::Old);
    assert_eq!(listing.files[1].state, FileState::Fetched);

    // Force downloads the unchanged file again and replaces its rows.
    let forced = FetchOptions {
        days: Some(7),
        ingest: IngestOptions {
            force: true,
            ..IngestOptions::default()
        },
    };
    let report = fx.fetch_with(&forced).unwrap();
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].status, FileStatus::Ingested);
    assert_eq!(fx.rows(), 20);

    // Without the limit the old file comes too.
    assert_eq!(fx.fetch().files.len(), 1);
    assert_eq!(fx.rows(), 30);
}

#[test]
fn a_short_download_ingests_nothing() {
    let mut fx = Fixture::new("remote-short");
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now_secs());
    fx.server.short_by.set(10);
    let error = fx.fetch_with(&FetchOptions::default()).unwrap_err();
    assert_eq!(error.code(), "remote_error");
    assert!(error.to_string().contains("run the fetch again"));
    assert_eq!(fx.rows(), 0);
    assert_eq!(fx.leftovers(), 0);
    assert_eq!(fx.states()[0].1, FileState::New);

    fx.server.short_by.set(0);
    fx.fetch();
    assert_eq!(fx.rows(), 20);
}

#[test]
fn cancelling_keeps_nothing() {
    let mut fx = Fixture::new("remote-cancel");
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now_secs());
    let result = fetch(
        &mut fx.store,
        &FetchOptions::default(),
        &fx.server,
        &mut |_| {},
        &AtomicBool::new(true),
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(fx.rows(), 0);
    assert_eq!(fx.leftovers(), 0);
}

#[test]
fn another_server_starts_afresh() {
    let mut fx = Fixture::new("remote-rehost");
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now_secs());
    fx.fetch();
    set(
        &fx.store,
        &Remote {
            host: "web2".into(),
            ..remote()
        },
    )
    .unwrap();
    assert_eq!(fx.states()[0].1, FileState::New);
}

#[test]
fn retention_none_does_not_alternate() {
    let mut fx = Fixture::with_settings(
        "remote-retention",
        SiteSettings {
            retention: Retention::None,
            ..SiteSettings::default()
        },
    );
    let now = now_secs();
    fx.server
        .put("/var/log/nginx/access.log.1", &log_lines(8, 30), now - DAY);
    fx.fetch();
    assert_eq!(fx.rows(), 30);

    // A new file replaces what was there, as an ingest would.
    fx.server
        .put("/var/log/nginx/access.log", &log_lines(10, 20), now);
    assert_eq!(fx.fetch().files.len(), 1);
    assert_eq!(fx.rows(), 20);

    // The earlier file is not fetched back in.
    assert!(fx.fetch().files.is_empty());
    assert_eq!(fx.rows(), 20);
}

/// A local path as a POSIX shell names it (Git's `sh` on Windows).
fn shell_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    match text.as_bytes() {
        [drive, b':', ..] if cfg!(windows) => {
            format!("/{}{}", (*drive as char).to_ascii_lowercase(), &text[2..])
        }
        _ => text,
    }
}

/// Runs the real scripts through a local `sh`, which is what the server
/// does with them. Skipped where there is no `sh` or the temporary folder
/// has a name the path rules refuse.
#[test]
fn scripts_run_in_a_real_shell() {
    let shell = Ssh { local_shell: true };
    let has_shell = Command::new("sh")
        .args(["-c", "exit 0"])
        .status()
        .is_ok_and(|s| s.success());
    let tmp = TempDataDir::new("remote-shell");
    let dir = tmp.0.root().join("logs");
    let base = shell_path(&dir);
    let remote = Remote {
        host: "local".into(),
        port: None,
        paths: vec![format!("{base}/*.log"), format!("{base}/missing-*")],
    };
    if !has_shell || remote.validate().is_err() {
        eprintln!("skipped: no usable sh for {base}");
        return;
    }
    std::fs::create_dir_all(&dir).unwrap();
    let content = log_lines(10, 40);
    std::fs::write(dir.join("a.log"), &content).unwrap();
    std::fs::write(dir.join("b.log"), b"").unwrap();
    std::fs::write(dir.join("c.txt"), b"not matched").unwrap();
    std::fs::write(dir.join("has space.log"), b"left out").unwrap();

    let (files, notes) = parse_listing(&shell.list(&remote).unwrap(), &remote).unwrap();
    let found: Vec<(&str, u64)> = files
        .iter()
        .map(|f| (f.path.rsplit('/').next().unwrap(), f.size))
        .collect();
    assert_eq!(found, [("a.log", content.len() as u64), ("b.log", 0)]);
    assert!((now_secs() - files[0].mtime).abs() < 3600);
    let notes = notes.join("\n");
    assert!(notes.contains("missing-*"), "{notes}");
    assert!(notes.contains("1 file(s) were left out"), "{notes}");

    let no_cancel = AtomicBool::new(false);
    let mut whole = Vec::new();
    let size = content.len() as u64;
    let got = shell
        .download(
            &remote,
            &files[0].path,
            size,
            &mut whole,
            &mut |_| {},
            &no_cancel,
        )
        .unwrap();
    assert_eq!(got, size);
    assert_eq!(whole, content.as_bytes());

    // Exactly the bytes asked for, even though the file is longer.
    let mut part = Vec::new();
    shell
        .download(
            &remote,
            &files[0].path,
            100,
            &mut part,
            &mut |_| {},
            &no_cancel,
        )
        .unwrap();
    assert_eq!(part, &content.as_bytes()[..100]);

    // A file that has gone is an error, with the shell's own words.
    let gone = format!("{base}/gone.log");
    let error = shell
        .download(&remote, &gone, 10, &mut Vec::new(), &mut |_| {}, &no_cancel)
        .unwrap_err();
    assert!(matches!(error, Error::Remote { .. }), "{error}");
}
