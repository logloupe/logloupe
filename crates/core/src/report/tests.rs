use std::sync::atomic::AtomicBool;

use chrono::NaiveDate;
use serde_json::{Value as Json, json};

use super::*;
use crate::ingest::{IngestOptions, ingest, prune_before};
use crate::store::testing::TempDataDir;
use crate::store::{OpenOptions, SiteSettings};

const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
const GOOGLEBOT: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";

fn line(ip: u8, when: &str, path: &str, status: u16, referer: &str, ua: &str) -> String {
    format!(
        "203.0.113.{ip} - - [{when} +0000] \"GET {path} HTTP/2.0\" {status} 100 \"{referer}\" \"{ua}\"\n"
    )
}

/// Two days of traffic on www.example.com. See the expectations in each test.
fn fixture(name: &str) -> (TempDataDir, Store) {
    let own = "https://www.example.com/";
    let log = [
        line(
            1,
            "14/Nov/2026:10:00:00",
            "/",
            200,
            "https://www.google.com/",
            CHROME,
        ),
        line(1, "14/Nov/2026:10:05:00", "/products/", 200, own, CHROME),
        line(1, "14/Nov/2026:10:06:00", "/style.css", 200, own, CHROME),
        line(1, "14/Nov/2026:11:00:00", "/contact/", 200, "-", CHROME),
        line(
            2,
            "14/Nov/2026:10:01:00",
            "/offer/?utm_source=news&utm_medium=email&utm_campaign=Spring+Sale%21&gclid=abc",
            200,
            "-",
            CHROME,
        ),
        line(
            3,
            "14/Nov/2026:10:02:00",
            "/guide/",
            200,
            "https://chatgpt.com/",
            CHROME,
        ),
        line(
            4,
            "14/Nov/2026:10:03:00",
            "/guide/?utm_source=chatgpt.com",
            200,
            "-",
            CHROME,
        ),
        line(5, "14/Nov/2026:10:04:00", "/products/", 200, "-", GOOGLEBOT),
        line(
            6,
            "15/Nov/2026:09:00:00",
            "/missing/",
            404,
            "https://www.bing.com/search?q=x",
            CHROME,
        ),
    ]
    .concat();

    let tmp = TempDataDir::new(name);
    let settings = SiteSettings {
        hosts: vec!["www.example.com".into()],
        ..SiteSettings::default()
    };
    let mut store = Store::create(&tmp.0, "site", &settings, &OpenOptions::default()).unwrap();
    let path = tmp.0.root().join("access.log");
    std::fs::write(&path, log).unwrap();
    ingest(
        &mut store,
        &[path],
        &IngestOptions::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    (tmp, store)
}

fn table<'a>(result: &'a ReportResult, name: &str) -> &'a Table {
    result
        .tables
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no table {name}"))
}

/// A table's rows as JSON objects keyed by column.
fn rows(result: &ReportResult, name: &str) -> Vec<Json> {
    let t = table(result, name);
    t.rows
        .iter()
        .map(|row| {
            Json::Object(
                t.columns
                    .iter()
                    .zip(row)
                    .map(|(c, v)| (c.key.to_owned(), v.clone()))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn overview() {
    let (_tmp, store) = fixture("report-overview");
    let result = run(&store, "overview", &ReportParams::default()).unwrap();
    assert_eq!(result.from, NaiveDate::from_ymd_opt(2026, 11, 14).unwrap());
    assert_eq!(result.to, NaiveDate::from_ymd_opt(2026, 11, 15).unwrap());
    assert_eq!(result.timezone, "UTC");
    assert_eq!(result.coverage.rollup_days, 0);

    let series = rows(&result, "series");
    assert_eq!(series.len(), 2);
    assert_eq!(
        series[0],
        json!({"bucket": "2026-11-14", "requests": 8, "pages": 6, "bytes": 800, "unique_ips": 5,
               "human": 7, "bot": 1, "s2xx": 8, "s3xx": 0, "s4xx": 0, "s5xx": 0})
    );
    assert_eq!(series[1]["s4xx"], 1);
    let totals = &rows(&result, "totals")[0];
    assert_eq!(
        (&totals["requests"], &totals["pages"], &totals["unique_ips"]),
        (&json!(9), &json!(6), &json!(6))
    );

    let hourly = ReportParams {
        granularity: Some(Granularity::Hour),
        ..Default::default()
    };
    let series = rows(&run(&store, "overview", &hourly).unwrap(), "series");
    assert_eq!(series.len(), 3);
    assert_eq!(series[0]["bucket"], "2026-11-14 10:00:00");
    assert_eq!(series[0]["requests"], 7);
}

#[test]
fn filters_dates_and_paging() {
    let (_tmp, store) = fixture("report-filters");
    let totals = |params: &ReportParams| {
        rows(&run(&store, "overview", params).unwrap(), "totals")[0]["requests"].clone()
    };
    let with = |filters: Filters| ReportParams {
        filters,
        ..Default::default()
    };

    assert_eq!(
        totals(&with(Filters {
            status_class: Some(4),
            ..Default::default()
        })),
        1
    );
    assert_eq!(
        totals(&with(Filters {
            ip: Some("203.0.113.1".into()),
            ..Default::default()
        })),
        4
    );
    assert_eq!(
        totals(&with(Filters {
            ip: Some("203.0.113.77/24".into()),
            ..Default::default()
        })),
        9
    );
    assert_eq!(
        totals(&with(Filters {
            path_prefix: Some("/guide".into()),
            ..Default::default()
        })),
        2
    );
    assert_eq!(
        totals(&with(Filters {
            source_type: Some("ai".into()),
            ..Default::default()
        })),
        2
    );
    let referrer = |text: &str| {
        with(Filters {
            referrer: Some(text.into()),
            ..Default::default()
        })
    };
    assert_eq!(totals(&referrer("(none)")), 4);
    assert_eq!(totals(&referrer("GOOGLE")), 1);
    assert_eq!(totals(&referrer("=https://www.example.com/")), 2);
    assert_eq!(totals(&referrer("=https://www.example.com")), 0);
    let agent = |text: &str| {
        with(Filters {
            user_agent: Some(text.into()),
            ..Default::default()
        })
    };
    assert_eq!(totals(&agent("googlebot/2.1")), 1);
    assert_eq!(totals(&agent("chrome/157")), 8);
    assert_eq!(totals(&agent("(none)")), 0);
    assert_eq!(
        totals(&with(Filters {
            bot: Some("googlebot".into()),
            ..Default::default()
        })),
        1
    );
    assert!(
        run(
            &store,
            "overview",
            &with(Filters {
                ip: Some("203.0.113.0/16".into()),
                ..Default::default()
            })
        )
        .is_err()
    );

    let day = NaiveDate::from_ymd_opt(2026, 11, 15);
    assert_eq!(
        totals(&ReportParams {
            from: day,
            to: day,
            ..Default::default()
        }),
        1
    );

    let paged = ReportParams {
        table: Some("by_source_type".into()),
        limit: 2,
        offset: 1,
        ..Default::default()
    };
    let result = run(&store, "sources", &paged).unwrap();
    assert_eq!(result.tables.len(), 1);
    let t = &result.tables[0];
    assert_eq!(
        (t.rows.len(), t.total_rows, t.offset, t.limit),
        (2, 5, 1, 2)
    );
    let past_end = ReportParams {
        offset: 50,
        ..paged.clone()
    };
    let t = &run(&store, "sources", &past_end).unwrap().tables[0];
    assert_eq!((t.rows.len(), t.total_rows), (0, 5));

    assert_eq!(
        run(&store, "nope", &ReportParams::default())
            .unwrap_err()
            .code(),
        "unknown_report"
    );
    let wrong_table = ReportParams {
        table: Some("nope".into()),
        ..Default::default()
    };
    assert_eq!(
        run(&store, "sources", &wrong_table).unwrap_err().code(),
        "invalid_argument"
    );
}

#[test]
fn sources() {
    let (_tmp, store) = fixture("report-sources");
    let result = run(&store, "sources", &ReportParams::default()).unwrap();
    assert_eq!(
        rows(&result, "by_source_type"),
        vec![
            json!({"source_type": "ai", "pages": 2, "requests": 2}),
            json!({"source_type": "direct", "pages": 1, "requests": 2}),
            json!({"source_type": "internal", "pages": 1, "requests": 2}),
            json!({"source_type": "search", "pages": 1, "requests": 2}),
            json!({"source_type": "campaign", "pages": 1, "requests": 1}),
        ]
    );
    assert_eq!(
        table(&result, "by_source_type").totals,
        json!({"pages": 6, "requests": 9})
            .as_object()
            .unwrap()
            .clone()
    );
    assert_eq!(
        rows(&result, "by_referrer_host"),
        vec![
            json!({"source_type": "ai", "referer_host": "chatgpt.com", "pages": 1, "requests": 1}),
            json!({"source_type": "search", "referer_host": "www.google.com", "pages": 1, "requests": 1}),
            json!({"source_type": "search", "referer_host": "www.bing.com", "pages": 0, "requests": 1}),
        ]
    );
    assert_eq!(
        rows(&result, "by_referrer_url")[2]["referer"],
        "https://www.bing.com/search?q=x"
    );
}

#[test]
fn campaigns() {
    let (_tmp, store) = fixture("report-campaigns");
    let result = run(&store, "campaigns", &ReportParams::default()).unwrap();
    assert_eq!(
        rows(&result, "campaigns"),
        vec![
            json!({"utm_source": "chatgpt.com", "utm_medium": null, "utm_campaign": null, "pages": 1, "requests": 1}),
            json!({"utm_source": "news", "utm_medium": "email", "utm_campaign": "Spring Sale!", "pages": 1, "requests": 1}),
        ]
    );
    assert_eq!(
        rows(&result, "click_ids"),
        vec![json!({"click_id": "gclid", "network": "Google Ads", "pages": 1, "requests": 1})]
    );
}

#[test]
fn landing_pages() {
    let (_tmp, store) = fixture("report-landing");
    let result = run(&store, "landing-pages", &ReportParams::default()).unwrap();
    // IP 1 has two sessions (10:00 and, after a 54-minute gap, 11:00).
    assert_eq!(
        rows(&result, "landing_pages"),
        vec![
            json!({"path": "/guide/", "sessions": 2}),
            json!({"path": "/", "sessions": 1}),
            json!({"path": "/contact/", "sessions": 1}),
            json!({"path": "/offer/", "sessions": 1}),
        ]
    );
    assert_eq!(table(&result, "landing_pages").totals["sessions"], 5);
    assert_eq!(
        rows(&result, "by_source_type")[0],
        json!({"source_type": "ai", "sessions": 2})
    );
}

#[test]
fn ai_referrals() {
    let (_tmp, store) = fixture("report-ai");
    let result = run(&store, "ai-referrals", &ReportParams::default()).unwrap();
    assert_eq!(
        rows(&result, "by_assistant"),
        vec![json!({"assistant": "chatgpt.com", "pages": 2, "requests": 2})]
    );
    assert_eq!(
        rows(&result, "by_landing_page"),
        vec![json!({"assistant": "chatgpt.com", "path": "/guide/", "pages": 2})]
    );
}

#[test]
fn pruned_days_fall_back_to_rollups_or_say_so() {
    let (_tmp, store) = fixture("report-pruned");
    let cutoff = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    assert_eq!(prune_before(&store, cutoff).unwrap(), 8);

    let result = run(&store, "overview", &ReportParams::default()).unwrap();
    assert_eq!(
        result.coverage,
        Coverage {
            raw_from: Some(cutoff),
            raw_to: Some(cutoff),
            rollup_days: 1
        }
    );
    let series = rows(&result, "series");
    assert_eq!(
        (&series[0]["bucket"], &series[0]["requests"]),
        (&json!("2026-11-14"), &json!(8))
    );
    assert_eq!(series[1]["requests"], 1);
    let totals = &rows(&result, "totals")[0];
    assert_eq!(
        (&totals["requests"], &totals["unique_ips"]),
        (&json!(9), &Json::Null)
    );
    assert!(result.notes.iter().any(|n| n.contains("daily rollups")));

    // A filter the rollup cannot answer: raw rows only, with a note.
    let filtered = ReportParams {
        filters: Filters {
            status_class: Some(4),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = run(&store, "overview", &filtered).unwrap();
    assert_eq!(rows(&result, "series").len(), 1);
    assert!(
        result
            .notes
            .iter()
            .any(|n| n.contains("It covers 2026-11-15 to 2026-11-15 only"))
    );

    let result = run(&store, "sources", &ReportParams::default()).unwrap();
    assert_eq!(table(&result, "by_source_type").totals["requests"], 1);
    assert!(result.notes.iter().any(|n| n.contains("pruned")));
}

#[test]
fn envelope_shape() {
    let (_tmp, store) = fixture("report-envelope");
    let result = run(&store, "sources", &ReportParams::default()).unwrap();
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["report"], "sources");
    assert_eq!(json["site"], "site");
    assert_eq!(json["from"], "2026-11-14");
    assert_eq!(json["schema_version"], 1);
    assert_eq!(
        json["coverage"],
        json!({"raw_from": "2026-11-14", "raw_to": "2026-11-15", "rollup_days": 0})
    );
    assert_eq!(
        json["tables"][0]["columns"][0],
        json!({"key": "source_type", "type": "string"})
    );
    assert_eq!(json["tables"][0]["rows"][0], json!(["ai", 2, 2]));
    assert!(json["generated_at"].as_str().unwrap().ends_with('Z'));
    assert!(json["notes"].as_array().is_some_and(|n| !n.is_empty()));
}

#[test]
fn csv_export_quotes_and_defuses_formulas() {
    let table = Table {
        name: "t",
        columns: vec![
            col("path", ColumnType::String),
            col("hits", ColumnType::Integer),
        ],
        rows: vec![
            vec![json!("/plain"), json!(3)],
            vec![json!("/a,b \"c\""), json!(-2)],
            vec![json!("=HYPERLINK(\"http://evil.example\")"), json!(1)],
            vec![json!("@SUM(1+1)"), Json::Null],
        ],
        totals: Map::new(),
        total_rows: 4,
        offset: 0,
        limit: 100,
    };
    assert_eq!(
        to_csv(&table),
        "path,hits\r\n/plain,3\r\n\"/a,b \"\"c\"\"\",-2\r\n\"'=HYPERLINK(\"\"http://evil.example\"\")\",1\r\n'@SUM(1+1),\r\n"
    );
}

#[test]
fn status_codes() {
    let (_tmp, store) = fixture("report-status");
    let result = run(&store, "status-codes", &ReportParams::default()).unwrap();
    let by_status = rows(&result, "by_status");
    assert_eq!(by_status.len(), 2);
    assert_eq!(
        (
            &by_status[0]["status"],
            &by_status[0]["hits"],
            &by_status[0]["meaning"]
        ),
        (&json!(200), &json!(8), &json!("OK"))
    );
    assert_eq!(
        (&by_status[1]["status"], &by_status[1]["hits"]),
        (&json!(404), &json!(1))
    );
    assert_eq!(
        table(&result, "by_status").totals,
        json!({"hits": 9, "errors": 1, "server_errors": 0})
            .as_object()
            .unwrap()
            .clone()
    );
    assert_eq!(
        rows(&result, "error_paths"),
        vec![
            json!({"path": "/missing/", "status": 404, "hits": 1, "ips": 1,
                    "first_seen": "2026-11-15 09:00:00", "last_seen": "2026-11-15 09:00:00"})
        ]
    );
    assert_eq!(rows(&result, "error_ips")[0]["ip"], "203.0.113.6");
    // One bucket per day and code: 200 on the 14th, 404 on the 15th.
    assert_eq!(rows(&result, "series").len(), 2);

    let exact = ReportParams {
        filters: Filters {
            status: Some(404),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        rows(&run(&store, "overview", &exact).unwrap(), "totals")[0]["requests"],
        1
    );
}

#[test]
fn queries_and_requests() {
    let (_tmp, store) = fixture("report-queries");
    let result = run(&store, "queries", &ReportParams::default()).unwrap();
    let shapes = rows(&result, "by_shape");
    assert_eq!(shapes.len(), 2);
    assert!(shapes.iter().any(|s| s["path"] == "/offer/"
        && s["query_shape"] == "utm_source=…&utm_medium=…&utm_campaign=…&gclid=…"
        && s["hits"] == 1));
    let params = rows(&result, "by_param");
    let utm_source = params
        .iter()
        .find(|p| p["parameter"] == "utm_source")
        .unwrap();
    assert_eq!(
        (
            &utm_source["hits"],
            &utm_source["paths"],
            &utm_source["longest_value"]
        ),
        (&json!(2), &json!(2), &json!(11))
    );
    assert_eq!(rows(&result, "by_path").len(), 2);

    let result = run(&store, "requests", &ReportParams::default()).unwrap();
    let all = rows(&result, "requests");
    assert_eq!(all.len(), 9);
    assert_eq!(
        (&all[0]["time"], &all[0]["path"], &all[0]["status"]),
        (
            &json!("2026-11-15 09:00:00"),
            &json!("/missing/"),
            &json!(404)
        )
    );
    assert_eq!(table(&result, "requests").totals["requests"], 9);
    let one_ip = ReportParams {
        filters: Filters {
            ip: Some("203.0.113.1".into()),
            ..Default::default()
        },
        limit: 2,
        ..Default::default()
    };
    let t = &run(&store, "requests", &one_ip).unwrap().tables[0];
    assert_eq!((t.rows.len(), t.total_rows), (2, 4));
}

#[test]
fn block_list_from_filters() {
    use super::blocklist::{By, Style, collect, format};
    let (_tmp, store) = fixture("report-blocklist");
    let params = ReportParams::default();
    assert_eq!(
        collect(&store, &params, By::Ip, 2).unwrap(),
        vec!["203.0.113.1".to_owned()]
    );
    assert_eq!(
        collect(&store, &params, By::Network, 1).unwrap(),
        vec!["203.0.113.0/24".to_owned()]
    );
    let all = collect(&store, &params, By::Ip, 1).unwrap();
    assert_eq!(all.len(), 6);
    assert_eq!(
        format(&all, Style::Nginx).unwrap().lines().next(),
        Some("deny 203.0.113.1;")
    );
}

#[test]
fn block_lists_never_include_verified_bots_or_local_addresses() {
    use super::blocklist::{By, collect, protect};
    let (_tmp, store) = fixture("report-blocklist-protect");
    // Mark the Googlebot request (203.0.113.5) as verified, as a real ingest would.
    store
        .conn
        .execute_batch("UPDATE requests SET bot_verify = 'verified' WHERE ip = '203.0.113.5'")
        .unwrap();
    let params = ReportParams::default();
    let ips = collect(&store, &params, By::Ip, 1).unwrap();
    assert_eq!(ips.len(), 5);
    assert!(!ips.contains(&"203.0.113.5".to_owned()));
    // Its whole network is protected too.
    assert!(collect(&store, &params, By::Network, 1).unwrap().is_empty());

    let picked = vec![
        "203.0.113.1".to_owned(),
        "203.0.113.5".to_owned(),
        "127.0.0.1".to_owned(),
        "203.0.113.0/24".to_owned(),
    ];
    let (kept, left_out) = protect(&store, &picked).unwrap();
    assert_eq!(kept, vec!["203.0.113.1".to_owned()]);
    assert_eq!(left_out.len(), 3);
}

#[test]
fn bursts_become_incidents() {
    // Ten quiet minutes at 3 requests a minute, with one address sending 100
    // requests at 10:03, 80 at 10:04, and after a gap 70 at 10:20.
    let mut log = String::new();
    for minute in 0..10 {
        for visitor in 0..3u8 {
            log += &format!(
                "198.51.100.{} - - [14/Nov/2026:10:{minute:02}:{:02} +0000] \"GET /page-{minute}/ HTTP/2.0\" 200 100 \"-\" \"{CHROME}\"\n",
                visitor + 1,
                visitor * 10
            );
        }
    }
    for (minute, count) in [(3, 100), (4, 80), (20, 70)] {
        for n in 0..count {
            log += &format!(
                "203.0.113.50 - - [14/Nov/2026:10:{minute:02}:{:02} +0000] \"POST /wp-login.php HTTP/2.0\" {} 50 \"-\" \"{CHROME}\"\n",
                n % 60,
                if n % 2 == 0 { 200 } else { 429 }
            );
        }
    }
    let tmp = TempDataDir::new("report-bursts");
    let mut store = Store::create(
        &tmp.0,
        "site",
        &SiteSettings::default(),
        &OpenOptions::default(),
    )
    .unwrap();
    let path = tmp.0.root().join("access.log");
    std::fs::write(&path, log).unwrap();
    ingest(
        &mut store,
        &[path],
        &IngestOptions::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();

    let result = run(&store, "bursts", &ReportParams::default()).unwrap();
    let incidents = rows(&result, "incidents");
    let by_address: Vec<&Json> = incidents
        .iter()
        .filter(|i| i["level"] == "address")
        .collect();
    assert_eq!(
        by_address.len(),
        2,
        "two bursts fifteen minutes apart are two incidents"
    );
    let first = by_address
        .iter()
        .find(|i| i["peak_per_minute"] == 100)
        .unwrap();
    assert_eq!(first["target"], "203.0.113.50");
    assert_eq!(
        (&first["start"], &first["end"]),
        (&json!("2026-11-14 10:03:00"), &json!("2026-11-14 10:04:59"))
    );
    assert_eq!(
        (&first["burst_minutes"], &first["requests"], &first["ips"]),
        (&json!(2), &json!(180), &json!(1))
    );
    assert_eq!(
        (&first["errors"], &first["blocked"], &first["top_path"]),
        (&json!(90), &json!(90), &json!("/wp-login.php"))
    );
    let second = by_address
        .iter()
        .find(|i| i["peak_per_minute"] == 70)
        .unwrap();
    assert_eq!(second["requests"], 70);

    // The first burst also lifts the whole site above three times its normal rate.
    assert!(
        incidents
            .iter()
            .any(|i| i["level"] == "site" && i["peak_per_minute"] == 103)
    );
    // One address alone is not a network or provider incident.
    assert!(
        !incidents
            .iter()
            .any(|i| i["level"] == "network" || i["level"] == "provider")
    );

    let id = first["incident"].clone();
    let mix: Vec<Json> = rows(&result, "status_mix")
        .into_iter()
        .filter(|s| s["incident"] == id)
        .collect();
    assert_eq!(mix.len(), 2);
    assert!(mix.iter().all(|s| s["hits"] == 90));
    assert!(
        rows(&result, "top_paths")
            .iter()
            .any(|p| p["incident"] == id && p["path"] == "/wp-login.php" && p["hits"] == 180)
    );
    assert!(
        result
            .notes
            .iter()
            .any(|n| n.contains("Normal for this site"))
    );

    // A higher explicit threshold leaves only the biggest minute.
    let strict = ReportParams {
        threshold: Some(90),
        ..Default::default()
    };
    let strict_rows = rows(&run(&store, "bursts", &strict).unwrap(), "incidents");
    assert_eq!(
        strict_rows
            .iter()
            .filter(|i| i["level"] == "address")
            .count(),
        1
    );

    // A quiet site has no incidents.
    let (_tmp2, quiet) = fixture("report-bursts-quiet");
    assert!(
        rows(
            &run(&quiet, "bursts", &ReportParams::default()).unwrap(),
            "incidents"
        )
        .is_empty()
    );
}

const GOOGLEBOT_PHONE: &str = "Mozilla/5.0 (Linux; Android 6.0.1; Nexus 5X Build/MMB29P) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Mobile Safari/537.36 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
const BINGBOT: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm) Chrome/116.0.1938.76 Safari/537.36";

/// Two days of crawling. Addresses .10 (Googlebot smartphone), .11 (Googlebot
/// desktop) and .12 (Bingbot) are verified; .13 claims Googlebot and is
/// unverifiable; .14 is a person.
fn crawl_fixture(name: &str) -> (TempDataDir, Store) {
    let log = [
        line(
            10,
            "14/Nov/2026:10:00:00",
            "/robots.txt",
            200,
            "-",
            GOOGLEBOT_PHONE,
        ),
        line(
            10,
            "14/Nov/2026:10:00:01",
            "/sitemap.xml",
            200,
            "-",
            GOOGLEBOT_PHONE,
        ),
        line(
            10,
            "14/Nov/2026:10:01:00",
            "/blog/first-post/",
            200,
            "-",
            GOOGLEBOT_PHONE,
        ),
        line(
            10,
            "14/Nov/2026:10:02:00",
            "/blog/first-post/",
            200,
            "-",
            GOOGLEBOT_PHONE,
        ),
        line(
            10,
            "15/Nov/2026:09:00:00",
            "/blog/first-post/",
            404,
            "-",
            GOOGLEBOT_PHONE,
        ),
        line(
            11,
            "14/Nov/2026:10:03:00",
            "/blog/?page=2",
            200,
            "-",
            GOOGLEBOT,
        ),
        line(11, "14/Nov/2026:10:04:00", "/old-page", 301, "-", GOOGLEBOT),
        line(
            11,
            "14/Nov/2026:10:05:00",
            "/assets/app.js",
            200,
            "-",
            GOOGLEBOT,
        ),
        line(12, "14/Nov/2026:10:06:00", "/robots.txt", 404, "-", BINGBOT),
        line(12, "14/Nov/2026:10:07:00", "/about/", 200, "-", BINGBOT),
        line(13, "14/Nov/2026:10:08:00", "/secret/", 200, "-", GOOGLEBOT),
        line(
            14,
            "14/Nov/2026:10:09:00",
            "/blog/first-post/",
            200,
            "-",
            CHROME,
        ),
    ]
    .concat();

    let tmp = TempDataDir::new(name);
    let mut store = Store::create(
        &tmp.0,
        "site",
        &SiteSettings::default(),
        &OpenOptions::default(),
    )
    .unwrap();
    let path = tmp.0.root().join("access.log");
    std::fs::write(&path, log).unwrap();
    ingest(
        &mut store,
        &[path],
        &IngestOptions::default(),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    store
        .conn
        .execute_batch(
            "UPDATE requests SET bot_verify = 'verified'
               WHERE ip IN ('203.0.113.10', '203.0.113.11', '203.0.113.12');
             UPDATE requests SET bot_verify = 'unverifiable' WHERE ip = '203.0.113.13';",
        )
        .unwrap();
    (tmp, store)
}

#[test]
fn crawl_counts_verified_search_crawlers() {
    let (_tmp, store) = crawl_fixture("report-crawl");
    let result = run(&store, "crawl", &ReportParams::default()).unwrap();
    assert_eq!(
        rows(&result, "by_bot"),
        vec![
            json!({"bot": "Googlebot", "operator": "Google", "hits": 8, "urls": 6, "bytes": 800,
                   "redirects": 1, "errors": 1,
                   "first_seen": "2026-11-14 10:00:00", "last_seen": "2026-11-15 09:00:00"}),
            json!({"bot": "Bingbot", "operator": "Microsoft Bing", "hits": 2, "urls": 2, "bytes": 200,
                   "redirects": 0, "errors": 1,
                   "first_seen": "2026-11-14 10:06:00", "last_seen": "2026-11-14 10:07:00"}),
        ]
    );
    assert_eq!(table(&result, "by_bot").totals["hits"], 10);
    // The unverifiable Googlebot request is left out, and the report says so.
    assert!(
        result.notes.iter().any(
            |n| n.starts_with("1 request(s) from search engine crawlers could not be verified")
        )
    );
    assert_eq!(
        rows(&result, "series"),
        vec![
            json!({"bucket": "2026-11-14", "bot": "Googlebot", "hits": 7}),
            json!({"bucket": "2026-11-14", "bot": "Bingbot", "hits": 2}),
            json!({"bucket": "2026-11-15", "bot": "Googlebot", "hits": 1}),
        ]
    );
    let directories = rows(&result, "directories");
    assert_eq!(
        directories
            .iter()
            .map(|d| (
                d["directory"].as_str().unwrap(),
                d["hits"].as_i64().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![("/", 4), ("/blog/", 4), ("/about/", 1), ("/assets/", 1)]
    );
    assert_eq!(directories[1]["share"], 40.0);
    assert_eq!(directories[1]["urls"], 2);

    // The bot filter picks one crawler; the verified filter replaces the default.
    let with = |filters: Filters| ReportParams {
        filters,
        ..Default::default()
    };
    let bing = run(
        &store,
        "crawl",
        &with(Filters {
            bot: Some("bingbot".into()),
            ..Default::default()
        }),
    )
    .unwrap();
    assert_eq!(rows(&bing, "by_bot").len(), 1);
    assert_eq!(rows(&bing, "by_bot")[0]["hits"], 2);
    let unverified = run(
        &store,
        "crawl",
        &with(Filters {
            verified: Some("unverifiable".into()),
            ..Default::default()
        }),
    )
    .unwrap();
    assert_eq!(table(&unverified, "by_bot").totals["hits"], 1);
}

#[test]
fn crawled_urls_and_response_times() {
    let (_tmp, store) = crawl_fixture("report-crawled-urls");
    let result = run(&store, "crawled-urls", &ReportParams::default()).unwrap();
    let urls = rows(&result, "urls");
    // Seven URLs: Google and Bing both fetched /robots.txt.
    assert_eq!(urls.len(), 7);
    assert_eq!(
        urls[0],
        json!({"path": "/blog/first-post/", "query": null, "hits": 3,
               "first_seen": "2026-11-14 10:01:00", "last_seen": "2026-11-15 09:00:00",
               "last_status": 404, "avg_bytes": 100})
    );
    assert!(
        urls.iter()
            .any(|u| u["path"] == "/blog/" && u["query"] == "page=2")
    );
    assert_eq!(table(&result, "urls").totals["urls"], 7);
    // Fewest fetches first, then the one fetched longest ago.
    let least = rows(&result, "least_crawled");
    assert_eq!(least[0]["path"], "/sitemap.xml");
    assert_eq!(least.last().unwrap()["path"], "/blog/first-post/");
    assert_eq!(rows(&result, "largest").len(), 7);
    // No response times in the log: no slowest table, and a note saying why.
    assert!(result.tables.iter().all(|t| t.name != "slowest"));
    assert!(result.notes.iter().any(|n| n.contains("no slowest URLs")));

    store
        .conn
        .execute_batch("UPDATE requests SET request_ms = 250 WHERE path = '/about/'")
        .unwrap();
    let result = run(&store, "crawled-urls", &ReportParams::default()).unwrap();
    assert_eq!(
        rows(&result, "slowest"),
        vec![json!({"path": "/about/", "query": null, "hits": 1, "avg_ms": 250, "max_ms": 250})]
    );
    let about = rows(&result, "urls")
        .into_iter()
        .find(|u| u["path"] == "/about/")
        .unwrap();
    assert_eq!(about["avg_ms"], 250);
}

#[test]
fn crawl_errors_redirects_and_inconsistent_urls() {
    let (_tmp, store) = crawl_fixture("report-crawl-errors");
    let result = run(&store, "crawl-errors", &ReportParams::default()).unwrap();
    assert_eq!(
        rows(&result, "errors"),
        vec![
            json!({"path": "/blog/first-post/", "query": null, "status": 404, "hits": 1, "bots": 1,
                   "first_seen": "2026-11-15 09:00:00", "last_seen": "2026-11-15 09:00:00"}),
            json!({"path": "/robots.txt", "query": null, "status": 404, "hits": 1, "bots": 1,
                   "first_seen": "2026-11-14 10:06:00", "last_seen": "2026-11-14 10:06:00"}),
        ]
    );
    let redirects = rows(&result, "redirects");
    assert_eq!(redirects.len(), 1);
    assert_eq!(
        (&redirects[0]["path"], &redirects[0]["status"]),
        (&json!("/old-page"), &json!(301))
    );
    // The post went from 200 to 404; robots.txt gave Google 200 and Bing 404.
    assert_eq!(
        rows(&result, "inconsistent"),
        vec![
            json!({"path": "/blog/first-post/", "query": null, "statuses": "200, 404", "hits": 3,
                   "last_status": 404,
                   "first_seen": "2026-11-14 10:01:00", "last_seen": "2026-11-15 09:00:00"}),
            json!({"path": "/robots.txt", "query": null, "statuses": "200, 404", "hits": 2,
                   "last_status": 404,
                   "first_seen": "2026-11-14 10:00:00", "last_seen": "2026-11-14 10:06:00"}),
        ]
    );
}

#[test]
fn crawl_waste_housekeeping_and_googlebot_split() {
    let (_tmp, store) = crawl_fixture("report-crawl-waste");
    let result = run(&store, "crawl-waste", &ReportParams::default()).unwrap();
    let summary: Vec<(String, i64)> = rows(&result, "summary")
        .iter()
        .map(|s| {
            (
                s["category"].as_str().unwrap().to_owned(),
                s["hits"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("page", 3),
            ("error", 2),
            ("housekeeping", 2),
            ("asset", 1),
            ("parameters", 1),
            ("redirect", 1)
        ]
        .map(|(c, h)| (c.to_owned(), h))
    );
    assert_eq!(rows(&result, "summary")[0]["share"], 30.0);
    assert_eq!(
        table(&result, "summary").totals,
        json!({"hits": 10, "wasted": 4})
            .as_object()
            .unwrap()
            .clone()
    );
    let patterns = rows(&result, "patterns");
    assert_eq!(patterns.len(), 4);
    assert!(
        patterns
            .iter()
            .any(|p| p["category"] == "parameters" && p["pattern"] == "/blog/?page=…")
    );

    let result = run(&store, "crawl-housekeeping", &ReportParams::default()).unwrap();
    let by_bot = rows(&result, "by_bot");
    assert_eq!(by_bot.len(), 3);
    assert_eq!(
        by_bot[0],
        json!({"bot": "Bingbot", "file": "robots.txt", "hits": 1, "ok": 0, "errors": 1,
               "last_status": 404,
               "first_seen": "2026-11-14 10:06:00", "last_seen": "2026-11-14 10:06:00"})
    );
    assert_eq!(
        (&by_bot[2]["bot"], &by_bot[2]["file"], &by_bot[2]["ok"]),
        (&json!("Googlebot"), &json!("sitemap"), &json!(1))
    );
    assert_eq!(rows(&result, "files").len(), 3);

    let result = run(&store, "googlebot", &ReportParams::default()).unwrap();
    let by_crawler = rows(&result, "by_crawler");
    assert_eq!(
        by_crawler
            .iter()
            .map(|c| (
                c["crawler"].as_str().unwrap(),
                c["hits"].as_i64().unwrap(),
                c["share"].as_f64().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("Googlebot Smartphone", 5, 62.5),
            ("Googlebot Desktop", 3, 37.5)
        ]
    );
    assert_eq!(rows(&result, "series").len(), 3);
}
