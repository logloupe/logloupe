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
