# Log Analyser — Build Spec

Working name: `log-analyser` (binary: `loga`). Both are placeholders.

A cross-platform tool for analysing web server access logs: where traffic comes from, what bots are doing, which bots are fake, suspected denial-of-service activity, and SEO crawl problems. It ships as a desktop app and as a command-line binary that shares the same engine.

---

## 1. Goals and non-goals

### Goals

- Ingest nginx and Apache/cPanel access logs quickly, with low and bounded memory use, regardless of file size.
- Answer questions GoAccess does not: verified vs fake bots, per-ASN traffic, burst detection, crawl analysis per URL.
- One Rust engine, two front ends: a Tauri desktop app (Windows, macOS, Linux) and a CLI that emits JSON (for use from Laravel on an Ubuntu server).
- History is the user's choice per site, with clear disk usage and easy clean-up.

### Non-goals (for now)

- Real-time tailing or live dashboards.
- Replacing GoAccess for hourly server reports.
- IIS / W3C Extended / load balancer log formats (the parser must be pluggable so these can be added).
- A long-running server daemon or HTTP API. The CLI is the server interface.
- Storing SSH private keys or passwords.

---

## 2. Architecture

A Cargo workspace with three crates. **All logic lives in `core`.** The other two are thin.

```
log-analyser/
├── Cargo.toml              # workspace
├── SPEC.md
├── crates/
│   ├── core/               # library: parse, enrich, store, report. No UI, no CLI code.
│   ├── cli/                # binary `loga`: argument parsing + JSON/table output
│   └── app/                # Tauri 2 app: src-tauri (Rust commands) + ui (web frontend)
├── fixtures/               # sample and synthetic logs for tests
└── .github/workflows/      # CI + release builds
```

Rules:

- A Tauri command and a CLI subcommand for the same feature must call the same `core` function and return the same serialisable struct.
- `core` never prints, never reads CLI args, and never assumes a GUI. It reports progress through a callback/channel.
- Every report is a function `fn(&Store, &ReportParams) -> Result<ReportResult>`.

### Technology choices

| Concern | Choice | Notes |
|---|---|---|
| Storage and queries | DuckDB, embedded (`duckdb` crate, `bundled` feature) | Columnar, compresses logs well, spills to disk under a memory cap |
| Desktop shell | Tauri 2 | System webview, small footprint |
| Frontend | TypeScript + Vite + Vue 3 with Nuxt UI in Vue mode (Tailwind 4) | Icons are bundled locally (`@iconify-json/lucide`) so the UI makes no network requests |
| Charts / tables | uPlot for time series; tables show one server-side page at a time | Never render more rows than one page; paging asks the engine for the next page |
| Gzip | `flate2` | Streaming decode |
| GeoIP / ASN | `maxminddb` reading local `.mmdb` files | See 5.2 |
| CLI | `clap` | |
| HTTP (range lists, sitemap) | `ureq` with rustls | Lighter than `reqwest` and blocking, which suits the engine. Send a descriptive User-Agent; at least one range endpoint rejects default library agents |

Use current stable versions of all crates; check them at build time rather than trusting versions remembered from training.

---

## 3. Data storage

### 3.1 Layout

A data directory (OS-appropriate via the `directories` crate; overridable with `--data-dir` or `LOGA_DATA_DIR` for server use):

```
<data-dir>/
├── config.toml             # global settings
├── sites/
│   ├── <site-slug>.duckdb  # one database file per site
│   └── ...
├── ranges/                 # cached bot IP range lists (JSON) + fetch timestamps
└── geo/                    # .mmdb files
```

**One DuckDB file per site.** Deleting a site means deleting a file, which frees disk immediately. Do not put all sites in one database.

DuckDB allows either one read-write process or several read-only processes per file, never both. So open a site read-write only while ingesting, re-enriching or pruning, and read-only for reports. While one process is writing, a report from another process cannot open the file. The default is to fail at once with a clear message, error code `site_locked` and exit code 75. `--wait-lock <seconds>` retries until the lock clears or the time runs out, so a Laravel queued job can either wait or release itself back onto the queue.

Set on every connection: `memory_limit` (default 512 MB, configurable), `temp_directory` inside the data dir, and a thread count (default: cores − 1, minimum 1).

### 3.2 Schema (per site database)

Treat this as the starting point; adjust types if measurement shows a better option.

```sql
-- Site-level settings and state, as key/value pairs:
-- schema_version, timezone (3.4), hostnames, retention, client_ip mode (4.2), edge_cache (4.2),
-- overlap policy (4.3), and the version or hash of each enrichment input
-- (bot registry, range lists, geo files, UA rules, source host lists)
-- last applied to the stored rows (5.4)
CREATE TABLE meta (key VARCHAR PRIMARY KEY, value VARCHAR);

-- One row per ingested file, used for de-duplication (4.3) and clean-up
CREATE TABLE ingested_files (
  file_id        INTEGER PRIMARY KEY,
  source         VARCHAR,     -- 'local' | 'ssh:<profile>'
  path           VARCHAR,
  size_bytes     UBIGINT,     -- size on disk (compressed size for gzip)
  bytes_consumed UBIGINT,     -- decompressed bytes ingested so far, ending on a line boundary; the resume offset
  head_hash      VARCHAR,     -- hash of the first 64 KB of decompressed content (or all of it, if shorter)
  tail_hash      VARCHAR,     -- hash of the 64 KB of decompressed content ending at bytes_consumed
  format         VARCHAR,
  proxied        BOOLEAN,     -- see 4.2
  first_ts       TIMESTAMP,
  last_ts        TIMESTAMP,
  rows_ok        UBIGINT,
  rows_bad       UBIGINT,
  rows_skipped   UBIGINT,     -- dropped by the overlap policy (4.3)
  ingested_at    TIMESTAMP
);

-- Distinct user agents, classified once
CREATE TABLE user_agents (
  ua_id     UINTEGER PRIMARY KEY,
  ua        VARCHAR,
  category  VARCHAR,   -- 'browser' | 'bot' | 'tool' | 'library' | 'empty' | 'unknown'
  family    VARCHAR,   -- 'Chrome', 'Googlebot', 'curl', ...
  bot_key   VARCHAR    -- matches a key in the bot registry (5.1), else NULL
);

-- One row per request
CREATE TABLE requests (
  ts            TIMESTAMP,   -- stored in UTC
  ts_local      TIMESTAMP,   -- the same instant as wall-clock time in the site's time zone (3.4)
  ip            VARCHAR,     -- client IP in canonical text form (compressed lowercase IPv6; IPv4-mapped IPv6 stored as IPv4)
  ip_net        VARCHAR,     -- the /24 (IPv4) or /48 (IPv6) containing `ip`, as a CIDR; used for grouping in 6.3
  peer_ip       VARCHAR,     -- the proxy's address when `ip` came from a forwarded-IP field (4.2), else NULL
  method        VARCHAR,
  path          VARCHAR,     -- without query string
  query         VARCHAR,     -- NULL if none
  protocol      VARCHAR,
  raw_request   VARCHAR,     -- the unparsed request string when it is malformed (4.1), else NULL
  status        USMALLINT,
  bytes         UBIGINT,
  referer       VARCHAR,     -- NULL for '-' or empty
  referer_host  VARCHAR,
  ua_id         UINTEGER,
  request_ms    UINTEGER,    -- NULL when the log has no timing field
  vhost         VARCHAR,     -- NULL when the log has no host field
  country       VARCHAR,     -- ISO code, NULL if unknown
  asn           UINTEGER,
  asn_org       VARCHAR,
  bot_verify    VARCHAR,     -- 'verified' | 'fake' | 'unverifiable' | NULL (not a bot claim)
  source_type   VARCHAR,     -- see 6.1
  is_page       BOOLEAN,     -- see 6.1
  file_id       INTEGER
);

-- Known URLs for crawl comparison (sitemap or imported list)
CREATE TABLE known_urls (
  path       VARCHAR,
  origin     VARCHAR,   -- 'sitemap' | 'import'
  lastmod    TIMESTAMP,
  added_at   TIMESTAMP
);

-- Small daily rollups that survive pruning of `requests`
CREATE TABLE daily_totals  (day DATE, requests UBIGINT, pages UBIGINT, bytes UBIGINT,
                            unique_ips UBIGINT, s2xx UBIGINT, s3xx UBIGINT,
                            s4xx UBIGINT, s5xx UBIGINT, human UBIGINT, bot UBIGINT);
CREATE TABLE daily_bots    (day DATE, bot_key VARCHAR, bot_verify VARCHAR,
                            hits UBIGINT, bytes UBIGINT, urls UBIGINT, errors UBIGINT);
CREATE TABLE daily_sources (day DATE, source_type VARCHAR, referer_host VARCHAR,
                            country VARCHAR, asn UINTEGER, hits UBIGINT, pages UBIGINT);
```

Rollups are rebuilt for every day touched by an ingest, in the same transaction as the insert. A rollup `day` is a day in the site's time zone (3.4).

`daily_sources` must stay small: keep the top 500 `referer_host` values per day and fold the rest into one `(other)` row per `source_type`.

Arbitrary CIDR filters on `ip` need a numeric comparison. Choose by measurement between an extra numeric column (`UHUGEINT`, IPv4 mapped into the IPv6 space) and DuckDB's `inet` extension, if that is available in the bundled build.

### 3.3 Retention and clean-up

Per-site setting, changeable at any time:

| Mode | Behaviour |
|---|---|
| `none` | Analyse then discard. Raw rows and rollups are cleared at the start of the next ingest, when the site is closed in the app, or by `loga db clear <slug>`. The CLI has no session, so server-side callers clear explicitly. |
| `days: N` | Raw rows older than N days are deleted after each ingest. Rollups are kept. |
| `forever` | Nothing is deleted automatically. |

Required operations (app and CLI):

- Show size on disk per site and in total.
- Delete a date range of raw rows (rollups optional, user chooses).
- Delete one ingested file's rows by `file_id`.
- Delete a whole site.
- Clear a site: delete all raw rows and rollups but keep the site and its settings.
- Compact: after large deletes, rewrite the database to a new file and swap it in, since deletes alone do not reliably shrink the file.

### 3.4 Time zone

Each site has an IANA time zone (`--timezone Europe/London`; default `UTC`). Timestamps are always stored in UTC. The site's time zone decides what a "day" and an "hour" are everywhere the user sees them: rollup days, per-day and per-hour report buckets, and the meaning of `--from` / `--to` dates. Every report envelope states the time zone used.

Changing a site's time zone rebuilds its rollups from raw rows. Days whose raw rows have been pruned cannot be rebuilt and keep their old boundaries; warn before the change and say which days are affected.

DuckDB's own time zone support depends on its ICU extension, which the `bundled` build does not include (it needs the slower `bundled-cmake` build). So local time is computed in Rust (`chrono-tz`) at ingest and stored in `requests.ts_local`. Reports bucket and filter on `ts_local`; `ts` stays UTC for everything else.

### 3.5 Raw rows and rollups

Reports read raw rows wherever they exist. Rollups are only a fallback for days whose raw rows have been pruned.

- Each report declares whether a rollup can answer it, and with which filters. Overview (6.1.1) falls back to `daily_totals`, bot overview (6.2.1) to `daily_bots`, and sources and geography (6.1.2, 6.1.4) to `daily_sources`. Every other report needs raw rows.
- When the requested range includes pruned days and the report can fall back, it uses rollups for those days and says so in `notes`.
- When it cannot fall back (no matching rollup, or a filter the rollup cannot answer, such as IP, CIDR or path prefix), the report covers only the days that still have raw rows. The envelope's `coverage` field (7.2) gives the range actually covered, and a note explains the gap. Never return a silently partial answer.
- Distinct counts (`unique_ips`, `urls`) cannot be added across days. Over raw rows, compute them exactly for the whole range. Over rollup days, show the per-day values and return `null` for the range total, with a note.

---

## 4. Ingest pipeline

```
Source (local file | .gz | later: ssh stream)
  → BufRead line stream
  → format parser (one line → RawRecord, zero-copy where possible)
  → enrichment (UA classify, bot verify, geo/ASN, source type)
  → batched insert via DuckDB Appender
  → rollup rebuild for affected days
```

Requirements:

- **Streaming only.** Never read a whole file into memory and never memory-map (gzip input rules that out). Every source is exposed as `impl BufRead`.
- Batch inserts (suggest 50–100k rows) inside one transaction per file. A failed or cancelled ingest leaves no partial rows.
- Parsing may be parallelised across chunks of lines, but insertion order need not be preserved.
- Bad lines are counted and the first 20 are kept for display; they never abort the ingest. Abort only if more than 20% of the first 1,000 lines fail, with a "wrong format?" error.
- Report progress as bytes consumed / total bytes (compressed bytes for gzip).
- De-duplicate as described in 4.3. Re-ingesting the same file is a no-op unless `--force`.
- Detect gzip by magic bytes, not file extension. Handle multi-member gzip.
- Accept multiple files and glob patterns in one call.

### 4.1 Log formats

A `LogFormat` trait with these implementations:

1. **Combined** (identical for default nginx `combined` and Apache `combined`, which is what cPanel domlogs use):
   `ip - user [10/Oct/2026:13:55:36 +0000] "GET /path?q=1 HTTP/1.1" 200 2326 "referer" "user-agent"`
   Hand-written parser, no regex, since this is the hot path. Note Apache writes `-` for zero bytes.
2. **Combined + extras**: the same with trailing fields. Recognise common additions: request time (nginx `$request_time` in seconds with decimals; Apache `%D` in microseconds), host/vhost, `X-Forwarded-For` / real client IP, upstream time.
3. **Custom pattern**: the user supplies an nginx `log_format` string or an Apache `LogFormat` string; compile it to a parser (a generated regex is acceptable here).

Auto-detection: sniff the first 200 non-empty lines against each known format and count the lines each one parses. A format qualifies if it parses at least 90% of them, which matches the tolerance ingest has for bad lines; a single line of scanner garbage must not fail detection. During detection, text left over after a format's last field counts as a mismatch, so plain combined does not claim a log that has trailing fields. Pick the qualifying format with the highest score, and on a tie the one that consumes the most fields. If none qualifies, ask the user (app) or exit with a message listing the formats tried and their scores (CLI).

Other parsing rules:

- Convert timestamps to UTC using the offset in the log line.
- Request lines may be malformed (scanners send garbage). Keep the row rather than discarding it, with `method`, `path`, `query` and `protocol` NULL and the unparsed request string in `raw_request`; these rows matter for security reports.
- Split path and query at the first `?`. Do not URL-decode for storage.
- IPv4 and IPv6 must both work everywhere.

### 4.2 Proxy / CDN check

This runs during the format sniff, before any enrichment, because every IP-based result depends on it. Sample the peer address of the first 1,000 lines. If more than 80% fall inside a CDN's published ranges (start with Cloudflare; cache the list as in 5.1), the file is `proxied`: the log is recording the proxy's address, not the visitor's.

- **The format has a forwarded-IP field** (`CF-Connecting-IP`, `X-Real-IP`, `X-Forwarded-For`): use it as the client IP, but only on lines whose peer address is inside the CDN's ranges. Anyone can send these headers straight to the origin, so a forwarded value from any other peer is ignored. For `X-Forwarded-For`, take the rightmost address that is not itself a CDN address. Keep the peer address in `peer_ip`.
- **The format has no such field**: ingest continues with a prominent warning that IP-based reports are meaningless until the server logs the real client IP. For these rows, bot claims are `unverifiable` (never `fake`), and country and ASN are NULL.
- A per-site `client_ip` setting (`auto`, the default; `peer`; `forwarded`) overrides the detection.

**Caching CDNs.** Restoring the client IP does not restore the traffic: behind a caching layer such as Cloudflare APO, the origin log holds only the requests that were not served from cache, so it under-counts visits and leans towards bots, spam and uncacheable pages (filtered searches, admin calls). A per-site `edge_cache` flag (`--edge-cache true`) makes every report carry a note saying so.

Raw lines are not stored, so a wrong choice here cannot be repaired by re-enrichment (5.4). It needs the server's log format fixed, or a re-ingest with `--force`.

### 4.3 Duplicate and overlapping files

The same lines reach the tool more than once in normal use: a live log ingested again after it has grown, the same log after rotation and compression, and cPanel live domlogs whose lines reappear later in the monthly archive. All fingerprints are taken over **decompressed** content, so `access.log.1` and `access.log.1.gz` are the same file.

Checks, in order, before any row is inserted:

1. **Same file.** `head_hash` matches a stored file, the content ending at that file's `bytes_consumed` matches its `tail_hash`, and nothing follows → no-op. With `--force`, delete that file's rows and ingest it again.
2. **Grown file.** As above, but more content follows → ingest only from `bytes_consumed` onwards and update the stored row (`bytes_consumed`, `tail_hash`, `last_ts`, counts). `bytes_consumed` always ends on a complete line; a half-written last line is left for next time. For gzip, reaching the offset means decompressing and discarding, which is far cheaper than parsing.
3. **Overlapping time range.** A different file whose lines fall inside the `first_ts`–`last_ts` range of a file already ingested for the site. The per-site `overlap` policy (overridable with `--overlap`) decides:
   - `error` (default): abort and roll back at the first overlapping line, naming the files that overlap. The app asks the user instead.
   - `skip`: drop lines inside a time range already covered and count them in `rows_skipped`. This is the right setting for a live log followed by its archive. Lines sharing the exact boundary second of an earlier file can be lost; accept that.
   - `allow`: ingest everything. For sites whose logs legitimately cover the same period: several servers, or cPanel's separate plain and `-ssl_log` domlogs for one domain.

---

## 5. Enrichment

Done per row at ingest, and repeatable later (5.4). Cache per-IP and per-UA results in memory for the duration of an ingest (IPs and UAs repeat heavily).

### 5.1 Bot registry and verification

A data file (TOML or JSON, shipped with the binary and overridable in the data dir) defines each known bot:

```toml
[[bot]]
key        = "googlebot"
name       = "Googlebot"
operator   = "Google"
kind       = "search"          # search | ai-training | ai-search | ai-user | seo-tool | monitor | social | other
ua_match   = ["Googlebot"]     # case-insensitive substrings
range_urls = ["..."]           # published CIDR lists
rdns       = ["googlebot.com", "google.com"]   # forward-confirmed reverse DNS suffixes
```

Verification order for a request whose UA claims a registered bot. The first rule that matches wins. Published ranges change, and logs can be months old, so `fake` is only given when the tool knows what the ranges were on the date of the request.

1. IP is inside one of the operator's current published ranges → `verified`.
2. IP is inside a retired prefix (see range handling) that was still published on the date of the request → `verified`.
3. The bot has `rdns` suffixes and DNS lookups are on → forward-confirmed reverse DNS (reverse lookup, suffix match, forward lookup returns the same IP). A pass → `verified`. A lookup error or timeout is not a negative.
4. No positive match, and the request is dated more than 30 days before the operator's `tracked_since` date → `unverifiable`: the tool has no record of the ranges at that time. Ranges change slowly, so a list is trusted to describe the 30 days before it was first fetched; without that grace a new install could never call anything in its first logs fake.
5. The bot has published ranges, or step 3 gave a clean negative → `fake`.
6. Otherwise (no ranges, and DNS off or failed) → `unverifiable`.

DNS lookups are optional (off by default in the CLI, on in the app), run only for IPs that reach step 3, once per distinct IP, and are cached. Rows from a proxied file with no real client IP skip all of this and are `unverifiable` (4.2).

Range sources to seed the registry with. The first four rows were already in production use in nginx bot-verification rules before this tool existed; reuse the same URLs and matching rules so the app and the servers agree on what counts as verified. They all share one JSON shape: `{"creationTime": "...", "prefixes": [{"ipv4Prefix": "..."}, {"ipv6Prefix": "..."}]}`.

| Operator | UA substrings (case-insensitive) | Source | Status |
|---|---|---|---|
| Google (common crawlers) | `googlebot` (covers Image/Video/News variants) | `https://developers.google.com/static/crawling/ipranges/common-crawlers.json` | In use. The old `/search/apis/ipranges/googlebot.json` path is dead. |
| Bing | `bingbot`, `msnbot`, `bingpreview`, `adidxbot` | `https://www.bing.com/toolbox/bingbot.json` | In use |
| OpenAI | `gptbot`, `oai-searchbot`, `chatgpt-user` | `https://openai.com/gptbot.json`, `https://openai.com/searchbot.json`, `https://openai.com/chatgpt-user.json` | In use |
| Anthropic | `claudebot`, `claude-user`, `claude-searchbot` | `https://claude.com/crawling/bots.json` (one list for all three), plus extra range `160.79.104.0/21` | In use. Confirmed 2026-11-15 that the list does not cover Claude-User's addresses. |
| Google (other) | `adsbot-google`, `storebot-google`, `mediapartners-google`, `google-inspectiontool`, and the user-triggered fetchers | `special-crawlers.json`, `user-triggered-fetchers.json` and `user-triggered-fetchers-google.json` under `https://developers.google.com/static/crawling/ipranges/`, merged with the common list | Confirmed 2026-11-15 |
| OpenAI ads | `oai-adsbot` | `https://openai.com/adsbot.json` (merged into the OpenAI operator) | Confirmed 2026-11-15 |
| Perplexity | `perplexitybot`, `perplexity-user` | `https://www.perplexity.com/perplexitybot.json`, `https://www.perplexity.com/perplexity-user.json` | Confirmed 2026-11-15 |
| Apple | `applebot` | `https://search.developer.apple.com/applebot.json` | Confirmed 2026-11-15 |
| Common Crawl | `ccbot` | `https://index.commoncrawl.org/ccbot.json` | Confirmed 2026-11-15 |
| Meta | `meta-externalagent`, `meta-externalfetcher`, `meta-webindexer`, `facebookexternalhit`, `facebookbot` | Routes announced by AS32934, from `whois.radb.net` (the method Meta's crawler documentation gives). Written in the registry as `whois://whois.radb.net/-i origin AS32934`. Meta's geofeed file is not used: its terms limit it to content-delivery purposes. | Confirmed 2026-11-15 |
| No usable list yet | Amazon (HTML page only), DuckDuckGo (list did not respond), Yandex and Baidu (rDNS only), SEO tools, social previews | | `unverifiable` until rDNS is wired in |

Matching rules carried over from those projects:

- **Verify at operator level, label at bot level.** OpenAI's three lists are merged for verification, so a GPTBot user agent arriving from a ChatGPT-User address is still `verified`. The specific bot name still comes from the user agent.
- **Google's non-Googlebot agents use different ranges.** AdsBot, Mediapartners and Google-InspectionTool must be checked against the special-crawler and user-triggered lists, never the common-crawler list, or they will be wrongly flagged as fake. Until those lists are wired in, mark these `unverifiable`, not `fake`.
- **Refuse suspicious updates.** Reject a fetched list with fewer prefixes than a per-operator minimum (20 for Google and Bing, 10 for the AI operators). For an operator with several source files, replace the cached set only if every file fetched and parsed; a partial merge would silently drop a whole crawler's ranges.
- Send an identifying User-Agent on fetches (the nginx updaters use `verified-bot-ip-updater/1.0`).

Range handling:

- `loga ranges update` (and a button in the app) fetches all lists, validates them, and stores them with a fetch timestamp. Auto-refresh if older than 24 hours when online. Work fully offline from the cache.
- Keep the previous copy when a fetch fails or returns something unparseable.
- Load ranges into a prefix trie / sorted interval structure for O(log n) lookups, IPv4 and IPv6.
- Keep history. Each cached prefix records `first_seen` (the first fetch that contained it) and, when an update drops it, `retired_at` instead of being deleted; it still verifies requests dated before that day. Record per operator a `tracked_since` date (the first successful fetch) and the date of every successful fetch, so historical logs can be judged against what was published at the time.
- **Extra ranges.** An operator entry may list `extra_ranges`: networks registered to the operator that its published lists leave out. Addresses in them are `verified`; they never make anything `fake`. Needed in practice: Claude-User requests come from `160.79.104.0/21`, which is registered to Anthropic (ARIN `ANTHR5-ARIN`) but absent from `bots.json`.
- Which list version verified a row is not stored. Re-enrichment (5.4) re-runs verification for a date range against the current lists and their history.

Also flag, as a separate report rather than a `bot_verify` value: browser-like user agents coming from data-centre ASNs at high volume (likely undeclared bots).

### 5.2 Country and ASN

Read local `.mmdb` files. Default to DB-IP Lite (country + ASN; free, no account, attribution required), with MaxMind GeoLite2 supported if the user supplies their own files or licence key. Confirm current licence terms and download URLs before wiring in an auto-download. `loga geo update` fetches the monthly files. If no database is present, ingest still works and the columns are NULL.

### 5.3 User-agent classification

Classify each distinct UA string once into `category`, `family`, and `bot_key`. A compact rule table is enough: the bot registry first, then known tools/libraries (curl, wget, python-requests, Go-http-client, headless markers), then major browser families. Empty or `-` UA is its own category.

### 5.4 Re-enrichment

Enriched columns go stale when their inputs change: the bot registry and range lists (`bot_verify`), geo files (`country`, `asn`, `asn_org`), UA rules (`user_agents`, and through it `is_page`), and the source host lists or the site's own hostnames (`source_type`). Rows ingested before a feature existed have NULLs in its columns.

`loga reenrich <slug> [--from DATE] [--to DATE] [--only ua,source,geo,bots]` recomputes those columns from the stored fields for a date range, then rebuilds the rollups for the affected days, in one transaction. The app has the same action on the Storage screen.

The `meta` table records the version or hash of each input last applied. `loga site show` and the app say when stored rows are behind the current inputs, and which `--only` groups would bring them up to date. Nothing re-enriches automatically.
---

## 6. Reports

Every report takes: site, date range, and optional filters (bot key, verified state, status class, path prefix, country, ASN, IP/CIDR). Every report returns the common envelope in 7.2. Every table can be exported to CSV.

### 6.1 Traffic sources (first deliverable)

Definitions set at ingest:

- `is_page`: method GET, status 200 or 304, path does not end in a static asset extension (configurable list), and UA category is `browser`. Logs record requests, not visits, so "pages" is an approximation and the UI must say so.
- `source_type`, from the referrer host and query string:
  - `direct` (no referrer), `internal` (referrer host is one of the site's own hostnames),
  - `search` (Google, Bing, DuckDuckGo, Yahoo, Yandex, Baidu, Ecosia, Brave…),
  - `ai` (chatgpt.com, perplexity.ai, claude.ai, gemini.google.com, copilot.microsoft.com…),
  - `social`, `email` (webmail hosts), `campaign` (`utm_source` or a click ID such as `gclid` / `fbclid` / `msclkid` present),
  - `other`.
  The host lists live in an editable data file.

Reports:

1. Overview: requests, pages, bytes, unique IPs, human/bot split, status classes, per hour or day.
2. Sources: pages by `source_type`, then by referrer host, then by full referrer URL.
3. Campaigns: `utm_source` / `utm_medium` / `utm_campaign` extracted from the query.
4. Geography and networks: by country, by ASN (with org name). ASN is the key view for spotting hosting-provider traffic posing as visitors.
5. Landing pages: first page per approximate session. Session = same IP + UA with gaps under 30 minutes.
6. AI referrals: the `ai` source type broken out by assistant and landing page.

### 6.2 Bots

1. Bot overview: hits, bytes, distinct URLs, error rate per bot, split by `kind`, per day.
2. Fake bots: requests claiming a registered bot from outside its ranges; grouped by claimed bot, IP, ASN, country; with the paths they targeted.
3. Bot IPs: activity per IP for a chosen bot.
4. AI crawlers: training vs search vs user-triggered fetches, per operator, with top URLs.
5. Undeclared automation: tools/libraries, empty UAs, and browser UAs from data-centre ASNs.

### 6.3 Abuse and denial of service

1. Bursts: requests per minute per IP, per /24 (IPv4) or /48 (IPv6), and per ASN. Flag windows above `max(absolute_floor, k × site median per-minute rate)`; both values configurable, with sensible defaults. Merge adjacent flagged minutes into incidents with start, end, peak rate, request count, top paths, and status mix.
2. Distributed patterns: one UA string or one target path receiving unusual volume from many IPs in a short window.
3. Expensive targets: hit rate on configurable costly endpoints (`/wp-login.php`, `/xmlrpc.php`, `/wp-admin/admin-ajax.php`, search URLs, cart/checkout).
4. Scanners: IPs with a high share of 4xx responses or requests for well-known probe paths (`.env`, `.git/`, `wp-config`, `phpmyadmin`…).
5. Server stress signals: spikes of 429, 499, 502, 503, 504.
6. For any incident, produce a copyable block list (CIDRs) in plain, nginx `deny`, and Apache `Require not ip` forms. The tool never applies blocks itself.

### 6.4 SEO and crawl analysis

Filtered to verified search bots by default, with a selector for which bot.

1. Crawled URLs: per URL, hit count, first seen, last seen, last status, average bytes, average response time (when available).
2. Crawl frequency: most and least crawled URLs and directories, crawls per day.
3. Errors seen by bots: 4xx and 5xx by URL.
4. Redirects seen by bots: 301/302/307/308 by URL.
5. Inconsistent responses: URLs returning more than one status class in the period.
6. Uncrawled URLs: in `known_urls` but with no verified bot hits in the period.
7. Orphan URLs: crawled by bots but not in `known_urls` (excluding assets and obvious junk).
8. Crawl waste: share of bot hits spent on parameterised URLs, redirects, errors, and assets; top wasteful URL patterns.
9. Large and slow URLs: by average bytes; by average response time when the log has timing. Hide the timing view when `request_ms` is entirely NULL and explain why.
10. Housekeeping: fetches of `robots.txt` and sitemaps per bot, with status codes.
11. Googlebot smartphone vs desktop split.

Known URLs come from: fetching the site's sitemap (follow sitemap index files, handle gzipped sitemaps), or importing a CSV/TXT with a URL column. Store paths normalised to match log paths.

### 6.5 Drill-down

From any row in any report, the user can open the matching raw requests, paged from the database (never loaded in full). Raw original lines are not stored; show the parsed fields.

---

## 7. CLI

### 7.1 Commands

```
loga site add <slug> --host example.com [--host www.example.com] [--timezone Europe/London]
                     [--retention forever|none|days:N] [--overlap error|skip|allow] [--client-ip auto|peer|forwarded]
                     [--edge-cache true|false]
loga site list | show <slug> | set <slug> [same options as add] | remove <slug>
loga ingest <slug> <files or globs...> [--format auto|combined|<name>] [--force] [--overlap error|skip|allow] [--no-dns]
loga report <slug> <report-name> [--from DATE] [--to DATE] [filters...] [--table NAME] [--limit N] [--offset N] [--format json|table|csv]
loga reports                       # list available report names, their tables and their parameters
loga urls <slug> sitemap [URL] | import <file> | clear
loga ranges update | status
loga geo update | status
loga db stats [<slug>] | files <slug> | prune <slug> --before DATE [--rollups] | delete-file <slug> <file-id>
        | clear <slug> | compact <slug>
loga reenrich <slug> [--from DATE] [--to DATE] [--only ua,source,geo,bots]
```

Global flags: `--data-dir`, `--memory-limit`, `--threads`, `--wait-lock <seconds>`, `--quiet`, `--json`, `--offline` (never fetch; use only the cached ranges).

`--json` makes every command other than `report` write one JSON document to stdout (site details, ingest results per file, storage figures). `report` uses its own `--format`, because `ingest --format` already means the log format.

Exit codes: 0 success, 1 failure, 2 usage error, 75 site locked by another process (3.1).

### 7.2 Output contract

`--format json` writes exactly one JSON document to stdout and nothing else. Progress and logs go to stderr. Exit code 0 on success, non-zero on failure with a JSON error object on stdout when `--format json` is set.

```json
{
  "report": "sources",
  "site": "example",
  "from": "2026-11-14",
  "to": "2026-11-14",
  "timezone": "Europe/London",
  "generated_at": "2026-11-15T16:40:00Z",
  "schema_version": 1,
  "coverage": { "raw_from": "2026-11-14", "raw_to": "2026-11-14", "rollup_days": 0 },
  "tables": [
    {
      "name": "by_source_type",
      "columns": [{ "key": "source_type", "type": "string" }, { "key": "pages", "type": "integer" }],
      "rows": [["search", 1234], ["direct", 987]],
      "totals": { "pages": 4321 },
      "total_rows": 9,
      "offset": 0,
      "limit": 100
    }
  ],
  "notes": ["Pages are estimated from request logs."]
}
```

Rules:

- A report returns one or more named tables. Overview returns a time series and a totals table; an incident report returns an `incidents` table plus child tables (`top_paths`, `status_mix`, `block_list`) that refer back to it through an `incident_id` column. Cells are always scalars, never nested objects.
- `--limit` and `--offset` apply per table, and `total_rows` is the count before paging. `--table NAME` returns just that table. `--format csv` writes one table: the one named, or the first.
- `coverage` says which days were answered from raw rows and how many from rollups (3.5).
- The error object is `{"schema_version": 1, "error": {"code": "site_locked", "message": "..."}}`. Codes are stable strings and part of the contract.

This envelope is the integration point for Laravel (invoke the binary from a queued job, decode stdout). Treat it as a public API: bump `schema_version` on breaking changes. The Tauri commands return the same structure.

---

## 8. Desktop app

- **Sites** screen: list of sites with date coverage, row count, size on disk, retention mode. Add/edit/delete.
- **Ingest**: drag and drop files onto a site, or pick them. Shows format detected, progress, rows accepted/rejected/skipped, sample bad lines, the CDN warning (4.2), and a prompt when a file overlaps one already ingested (4.3).
- **Reports**: left navigation grouped as Traffic, Bots, Abuse, SEO. A shared date range picker and filter bar. Each report is a chart (where it makes sense) above a sortable, virtualised table with CSV export and drill-down.
- **Storage** screen: per-site and total disk usage, prune by date, delete by ingested file, clear, compact, delete site, and re-enrich (5.4) with a notice when stored rows are behind the current inputs.
- **Settings**: memory limit, threads, DNS lookups on/off, range list status and refresh, geo database status and refresh, editable thresholds and host lists.
- Long operations run off the UI thread, report progress through Tauri events, and can be cancelled.
- The frontend never holds a full result set. Tables request pages of rows.
- Light and dark themes following the OS.

---

## 9. Remote log collection (later phase)

- Shell out to the system `ssh` client (present on Windows 10+, macOS, Linux). This reuses the user's `~/.ssh/config`, agent, and known hosts. The app stores **no keys and no passwords**.
- A remote profile is: an SSH host alias, one or more remote path globs, and the site it feeds.
- Fetch by running a remote `cat` (or `zcat`-free: stream the raw `.gz` bytes and decompress locally) and piping stdout straight into the ingest pipeline. Nothing is written to the remote server.
- List remote files first (name, size, mtime) and skip those already ingested.
- Typical locations to suggest: nginx `/var/log/nginx/*.log*`; cPanel live logs under `/usr/local/apache/domlogs/` or `/etc/apache2/logs/domlogs/` and monthly archives in `~/logs/<domain>-<Mon>-<YYYY>.gz`. Verify per server; these vary.

---

## 10. Privacy

Access logs contain IP addresses, which are personal data under UK GDPR, and these are client sites. Provide: the retention controls in 3.3, an option to truncate IPs (/24, /48) in rollups and exports, and no telemetry of any kind. The only outbound requests the tool makes are range list updates, geo database updates, sitemap fetches, and DNS lookups, each of which can be disabled.

---

## 11. Performance

The aim is to be as fast as possible with low, bounded memory use; that is why the engine is Rust and DuckDB. There are no fixed pass/fail times. Bigger files take longer, and that is fine as long as time grows in line with size and memory does not grow at all.

Hard requirements:

- Peak resident memory during ingest is flat with respect to file size: a 10 GB log uses no more than a 1 GB log. With the default 512 MB DuckDB limit, expect the whole process to stay around 600 MB; measure it.
- Reports respect the same memory limit and spill to disk rather than fail.
- The app never holds a full result set (section 8).

Measured and tracked, using the synthetic log generator in the repo:

- Ingest throughput (MB/s and rows/s) for plain and gzip input at 1 GB and 10 GB. It should scale linearly.
- Time for every report over 30 days of a 1 M requests/day site. When a report is slow, look for a better query or an extra rollup (for example per-minute counts per IP for 6.3) before accepting it.
- App idle memory with one site open.
- Database size as a share of the uncompressed log size.

Record the measured numbers in the README after each phase, and treat a clear regression between phases as a bug to investigate.

---

## 12. Testing

- Unit tests for every log format, including malformed request lines, IPv6, `-` byte counts, odd time zones, and very long lines.
- Golden-file tests: small fixture logs with hand-verified expected JSON for each report.
- Bot verification tests against frozen copies of range lists (no network in tests).
- An end-to-end test: ingest fixture → run every report through the CLI → validate the JSON envelope.
- A synthetic log generator (configurable size, bot mix, an injected burst, a fake Googlebot) used for both benchmarks and tests.
- `cargo clippy` and `cargo fmt --check` clean in CI.

---

## 13. Build and release

- GitHub Actions matrix on push: test on `windows-latest`, `macos-latest`, `ubuntu-latest`.
- On tag: build the Tauri app for Windows (x64), macOS (universal or arm64 + x64), Linux (AppImage/deb); build the standalone `loga` CLI for the same targets plus `x86_64-unknown-linux-gnu` for the Ubuntu server. Attach all to a GitHub release.
- macOS builds are unsigned at first. Document the first-launch override. Signing and notarisation can be added later with an Apple Developer account.
- Windows builds are unsigned at first; document the SmartScreen prompt.
- Development prerequisites to document in the README: Rust stable, Node LTS, Tauri's per-OS prerequisites (on Windows: MSVC build tools and WebView2).

---

## 14. Phases

Each phase ends with working software and passing tests. Do not start the next phase until the acceptance checks pass.

**Phase 1 — Engine and CLI, traffic sources**
- Workspace, `core`, `cli`. Site management, including time zone. Combined and combined + extras parsers (timing, vhost and forwarded-IP fields) with auto-detect. Local file ingest (plain and gzip). Schema, rollups, de-duplication and overlap handling (4.3). UA classification and `source_type` / `is_page`. Reports 6.1 (1–3, 5–6). JSON/table/CSV output, locking and exit codes.
- Accept: ingest a real daily nginx log and a real cPanel log; ingesting a grown copy of a log adds only the new lines, and its gzipped copy is a no-op; `loga report <site> sources --format json` returns a valid envelope; ingest throughput and peak memory measured and recorded.

**Phase 2 — Enrichment**
- Bot registry, range fetch/cache with history, verification, optional rDNS. Geo/ASN lookup. Proxy / CDN check and client IP selection (4.2). Custom log format strings. Re-enrichment (5.4). Reports 6.1 (4) and 6.2.
- Accept: a fixture containing a fake Googlebot is flagged `fake`; a real Googlebot IP is `verified`; a proxied fixture with no forwarded-IP field yields `unverifiable`, not `fake`; `loga reenrich` fills geo and bot columns on rows ingested in Phase 1; works offline from cache.

**Phase 3 — Desktop app**
- Tauri shell, sites, ingest with progress and cancel, report views for everything built so far, storage screen with retention, prune, compact, delete.
- Accept: installs and runs on Windows and macOS from CI artefacts; a 1 GB ingest keeps the UI responsive.

**Phase 4 — Abuse and SEO reports**
- Reports 6.3 and 6.4. Known URLs via sitemap and import. Drill-down.
- Accept: the synthetic burst is detected as one incident with a correct block list; uncrawled and orphan reports match a hand-built fixture.

**Phase 5 — Remote collection and polish**
- SSH profiles and fetch. Monthly archive handling at scale. IP truncation option. Release workflow.

---

## 15. Before starting

1. Put two or three real (anonymised) sample lines from each server type into `fixtures/`: plain nginx, any nginx with a custom `log_format`, and a cPanel domlog. The formats in this spec are assumed from defaults and must be checked against these.
2. Note which sites sit behind Cloudflare or another proxy and whether their logs record the real client IP.
3. Decide the final project/binary name. (Frontend framework decided: Vue 3 + Nuxt UI.)
4. Note each site's time zone (3.4), and whether it is fed by more than one server, which decides its overlap policy (4.3).
