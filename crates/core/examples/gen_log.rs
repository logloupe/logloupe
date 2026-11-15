//! Synthetic access log generator for benchmarks and tests.
//!
//! `cargo run --release -p logloupe-core --example gen_log -- --size 1GB --out big.log`
//!
//! Options: `--size <N>[KB|MB|GB]` or `--lines <N>`, `--out <file>` (default
//! stdout), `--days <N>` (default 1), `--start <YYYY-MM-DD>` (default
//! 2026-11-01), `--seed <N>`.
//!
//! The output is deterministic for a given seed. It contains a browser / bot /
//! tool mix, a fake Googlebot (Googlebot user agent from an address outside
//! Google's ranges), and one burst from a single address about 40% of the way
//! through.

use std::fs::File;
use std::io::{self, BufWriter, Write};

/// xorshift64*: small, fast and good enough for test data.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// True `per_mille` times in a thousand.
    fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }

    /// Skewed towards low numbers, like page and visitor popularity.
    fn skewed(&mut self, n: u64) -> u64 {
        let a = self.below(n);
        let b = self.below(n);
        a.min(b).min(self.below(n))
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const BROWSERS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.2 Mobile/15E148 Safari/604.1",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.6.1 Safari/605.1.15",
    "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Mobile Safari/537.36",
    "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/31.0 Chrome/148.0.0.0 Mobile Safari/537.36",
];

/// (user agent, IPv4 prefix the bot really crawls from)
const BOTS: &[(&str, &str)] = &[
    (
        "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
        "66.249.66",
    ),
    (
        "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm) Chrome/116.0.1938.76 Safari/537.36",
        "157.55.39",
    ),
    (
        "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; GPTBot/1.2; +https://openai.com/gptbot)",
        "20.171.207",
    ),
    (
        "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaudeBot/1.0; +claudebot@anthropic.com)",
        "160.79.104",
    ),
    (
        "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot) Chrome/119.0.6045.214 Safari/537.36",
        "52.70.240",
    ),
    (
        "Mozilla/5.0 (compatible; YandexBot/3.0; +http://yandex.com/bots)",
        "5.255.231",
    ),
    (
        "Mozilla/5.0 (compatible; AhrefsBot/7.0; +http://ahrefs.com/robot/)",
        "54.36.148",
    ),
];

const TOOLS: &[&str] = &[
    "curl/8.5.0",
    "python-requests/2.32.3",
    "Go-http-client/2.0",
    "Wget/1.21.4",
    "-",
];

const REFERRERS: &[&str] = &[
    "https://www.google.com/",
    "https://www.google.co.uk/",
    "https://www.bing.com/",
    "https://duckduckgo.com/",
    "https://chatgpt.com/",
    "https://www.perplexity.ai/",
    "https://www.facebook.com/",
    "https://t.co/abc123",
    "https://news.example.org/article/42",
];

const ASSETS: &[&str] = &[
    "/wp-content/themes/site/style.css",
    "/wp-content/themes/site/app.js",
    "/wp-content/uploads/2026/10/hero-1600x900.webp",
    "/wp-content/uploads/2026/10/gallery-500x332.jpeg",
    "/favicon.ico",
    "/wp-includes/js/jquery/jquery.min.js",
];

const PROBES: &[&str] = &[
    "/wp-login.php",
    "/xmlrpc.php",
    "/.env",
    "/.git/config",
    "/wp-config.php.bak",
    "/phpmyadmin/",
];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, usize, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month as usize, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn parse_size(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    let (digits, scale) = [("gb", 1u64 << 30), ("mb", 1 << 20), ("kb", 1 << 10)]
        .iter()
        .find_map(|(suffix, scale)| lower.strip_suffix(suffix).map(|d| (d.to_owned(), *scale)))
        .unwrap_or((lower.clone(), 1));
    digits.trim().parse::<u64>().ok().map(|n| n * scale)
}

struct Config {
    max_bytes: u64,
    max_lines: u64,
    out: Option<String>,
    days: u64,
    start_day: i64,
    seed: u64,
}

fn config() -> Result<Config, String> {
    let mut cfg = Config {
        max_bytes: u64::MAX,
        max_lines: u64::MAX,
        out: None,
        days: 1,
        start_day: days_from_civil(2026, 11, 1),
        seed: 42,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let bad = || format!("invalid value for {flag}: {value}");
        match flag.as_str() {
            "--size" => cfg.max_bytes = parse_size(&value).ok_or_else(bad)?,
            "--lines" => cfg.max_lines = value.parse().map_err(|_| bad())?,
            "--out" => cfg.out = Some(value),
            "--days" => cfg.days = value.parse().ok().filter(|d| *d > 0).ok_or_else(bad)?,
            "--seed" => cfg.seed = value.parse().map_err(|_| bad())?,
            "--start" => {
                let parts: Vec<i64> = value.split('-').filter_map(|p| p.parse().ok()).collect();
                let [y, m, d] = parts[..] else {
                    return Err(bad());
                };
                cfg.start_day = days_from_civil(y, m, d);
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if cfg.max_bytes == u64::MAX && cfg.max_lines == u64::MAX {
        return Err("give --size or --lines".into());
    }
    Ok(cfg)
}

fn main() -> io::Result<()> {
    let cfg = match config() {
        Ok(cfg) => cfg,
        Err(message) => {
            eprintln!("gen_log: {message}");
            std::process::exit(2);
        }
    };
    let sink: Box<dyn Write> = match &cfg.out {
        Some(path) => Box::new(File::create(path)?),
        None => Box::new(io::stdout().lock()),
    };
    let mut out = BufWriter::with_capacity(1 << 20, sink);
    let mut rng = Rng(cfg.seed.max(1));

    // Lines average about 230 bytes; used to spread timestamps over the period.
    let expected_lines = cfg.max_lines.min(cfg.max_bytes / 230).max(1);
    let span = cfg.days * 86_400;
    let burst_start = expected_lines * 2 / 5;
    let burst_len = (expected_lines / 200).clamp(50, 50_000);
    let start = cfg.start_day * 86_400;

    let (mut bytes, mut lines) = (0u64, 0u64);
    let mut line = String::with_capacity(512);
    while bytes < cfg.max_bytes && lines < cfg.max_lines {
        let ts = start + (lines as u128 * span as u128 / expected_lines as u128) as i64;
        let ts = ts.min(start + span as i64 - 1);
        let (year, month, day) = civil(ts.div_euclid(86_400));
        let second = ts.rem_euclid(86_400);

        let in_burst = (burst_start..burst_start + burst_len).contains(&lines);
        let roll = rng.below(1000);
        let (ip, ua, path, referer, status): (String, &str, String, String, u16);
        if in_burst {
            // One address hammering the login page.
            ip = "203.0.113.44".to_owned();
            ua = BROWSERS[0];
            path = "/wp-login.php".to_owned();
            referer = "-".to_owned();
            status = if rng.chance(900) { 200 } else { 429 };
        } else if roll < 10 {
            // Fake Googlebot: the user agent from an address Google does not own.
            ip = format!("45.148.10.{}", rng.below(8) + 1);
            ua = BOTS[0].0;
            path = rng.pick(PROBES).to_owned();
            referer = "-".to_owned();
            status = if rng.chance(700) { 404 } else { 200 };
        } else if roll < 260 {
            let (bot_ua, prefix) = BOTS[rng.skewed(BOTS.len() as u64) as usize];
            ip = format!("{prefix}.{}", rng.below(250) + 1);
            ua = bot_ua;
            path = match rng.below(100) {
                0..=1 => "/robots.txt".to_owned(),
                2 => "/sitemap_index.xml".to_owned(),
                3..=12 => format!("/products/?page={}&sort=price", rng.below(400)),
                _ => format!("/products/item-{}/", rng.skewed(20_000)),
            };
            referer = "-".to_owned();
            status = match rng.below(100) {
                0..=5 => 301,
                6..=8 => 404,
                9 => 500,
                _ => 200,
            };
        } else if roll < 310 {
            ip = format!(
                "{}.{}.{}.{}",
                80 + rng.below(40),
                rng.below(256),
                rng.below(256),
                rng.below(254) + 1
            );
            ua = rng.pick(TOOLS);
            path = if rng.chance(400) {
                rng.pick(PROBES).to_owned()
            } else {
                "/".to_owned()
            };
            referer = "-".to_owned();
            status = if rng.chance(500) { 404 } else { 200 };
        } else {
            // A visitor. Popular visitors and pages come up more often.
            let visitor = rng.skewed(200_000);
            ip = if visitor.is_multiple_of(20) {
                format!(
                    "2a01:4f8:{:x}:{:x}::{:x}",
                    visitor % 4096,
                    visitor / 4096,
                    visitor % 251 + 1
                )
            } else {
                format!(
                    "{}.{}.{}.{}",
                    20 + visitor % 180,
                    (visitor / 7) % 256,
                    (visitor / 13) % 256,
                    visitor % 254 + 1
                )
            };
            ua = BROWSERS[(visitor % BROWSERS.len() as u64) as usize];
            let asset = rng.chance(350);
            path = if asset {
                rng.pick(ASSETS).to_owned()
            } else {
                let page = format!("/products/item-{}/", rng.skewed(20_000));
                match rng.below(100) {
                    0..=1 => format!(
                        "{page}?utm_source=newsletter&utm_medium=email&utm_campaign=Autumn+Sale"
                    ),
                    2 => format!("{page}?gclid=Cj0KCQiA{:x}", rng.next()),
                    3..=9 => "/".to_owned(),
                    _ => page,
                }
            };
            referer = if asset || rng.chance(450) {
                "https://www.example.com/products/".to_owned()
            } else if rng.chance(250) {
                rng.pick(REFERRERS).to_owned()
            } else {
                "-".to_owned()
            };
            status = match rng.below(100) {
                0..=3 => 304,
                4..=6 => 301,
                7..=8 => 404,
                _ => 200,
            };
        }

        let size = if status == 200 {
            500 + rng.skewed(180_000)
        } else {
            rng.below(600)
        };
        line.clear();
        use std::fmt::Write as _;
        let _ = writeln!(
            line,
            "{ip} - - [{day:02}/{}/{year}:{:02}:{:02}:{:02} +0000] \"GET {path} HTTP/2.0\" {status} {size} \"{referer}\" \"{ua}\"",
            MONTHS[month - 1],
            second / 3600,
            second / 60 % 60,
            second % 60,
        );
        out.write_all(line.as_bytes())?;
        bytes += line.len() as u64;
        lines += 1;
    }
    out.flush()?;
    eprintln!("gen_log: wrote {lines} lines, {:.1} MB", bytes as f64 / 1e6);
    Ok(())
}
