# Logloupe: status and to-do

The single list of what is done and what is left. Update it in the same commit as the work: tick the item, move it if priorities change, and add anything new that comes up. The design behind each item is in [spec.md](spec.md); section numbers below refer to it.

Last updated: 2026-11-16, at commit `eb641fc`.

## Where things stand

- Engine, command-line tool and desktop app all work on Windows. CI passes on Windows, macOS and Linux (build and tests only; the app has never been opened on macOS or Linux).
- The owner uses the installed app on real client logs.
- The repository is `logloupe/logloupe` on GitHub, **private**. Nothing is released. The website at logloupe.com is a "coming soon" page built in a separate session.
- 79 tests, all passing. `cargo clippy` is clean with warnings as errors.

## Next up, in order

- [ ] **Data-centre browsers in Undeclared** (6.2.5). Now that ASN lookup exists, flag browser user agents coming from hosting providers at volume. Needs a way to tell hosting ASNs from access ISPs; decide that first.
- [ ] **Custom log formats** (4.1): "combined + extras" (request time, vhost, forwarded IP) and user-supplied nginx `log_format` / Apache `LogFormat` strings, set per site. **Blocked on the owner** supplying sample logs in a non-default format and a cPanel domlog.
- [ ] **Proxy / CDN detection** (4.2): detect logs that record Cloudflare's address instead of the visitor's, and use a forwarded-IP field when the format has one. Depends on custom formats.
- [ ] **SEO crawl reports** (6.4): crawled URLs, crawl frequency, errors and redirects seen by bots, inconsistent responses, crawl waste, housekeeping fetches, Googlebot smartphone vs desktop. The owner wants SEO as a secondary audience, so this matters for positioning.
- [ ] **Known URLs** (6.4): fetch a sitemap or import a URL list, then "uncrawled" and "orphan" reports. The `known_urls` table exists and is unused.
- [ ] **Release workflow** (13): GitHub Actions that builds installers for Windows, macOS and Linux on a tag and attaches them to a GitHub release.
- [ ] **Updater**: Tauri's updater plugin, checking GitHub releases. Needs the repository public and a signing key pair (the private key is the owner's to hold). Must stay opt-in: the app promises to make no requests the user did not start.
- [ ] **Server version**: an HTTP server exposing the same operations, serving the same frontend, with login (password first, then OpenID Connect) and a read-only API with tokens. Reads logs only from configured folders. The spec lists this as a non-goal and needs changing when work starts.
- [ ] **API / MCP for AIdata**: comes almost free with the server version. Until then AIdata can call the `logloupe` binary and read its JSON.

## Waiting on the owner

- [ ] Sample logs: a custom nginx `log_format`, and a cPanel domlog.
- [ ] Decide whether to squash commit `7eca532`, which does not compile (fixed by `8f6ff0a`). Needs a force-push.
- [ ] Decide when the repository goes public. Before that, have someone other than the author re-read it for client-identifying strings (two rounds of clean-up were needed already).
- [ ] Code signing: unsigned installers trigger a SmartScreen warning for anyone who downloads them. Needs a paid certificate; macOS needs an Apple Developer account.
- [ ] Confirm in the installed app: the second launch brings the window to the front (single-instance), and data moved across after the rename.

## Not yet verified

Things built and tested in the engine or from the command line, but not exercised by hand in the app:

- [ ] Dark mode on every screen (the chart is the likeliest problem).
- [ ] CSV export, the Copy button on block lists, "View requests", the Geography and Status codes pages, the Settings downloads.
- [ ] Reverse DNS running from inside the app (only run from the CLI so far).
- [ ] The app on macOS and Linux.
- [ ] Performance re-measured since bot verification, geo lookup and reverse DNS were added to ingest. Last figures (1 GB in about 25 s, 681 MB peak) predate them. Also unmeasured: gzip input, a 10 GB file, a 30-day site.

## Smaller gaps

- [ ] Rollup fallback for pruned days exists only for the Overview report (3.5). Sources, geography and bots report on remaining raw rows and say so.
- [ ] `reenrich` takes no date range and does not recompute `source_type` after hostnames change (5.4). Re-ingest with `--force` is the workaround.
- [ ] Arbitrary CIDR filters: only an exact address, an IPv4 /24 or an IPv6 /48 work (3.2).
- [ ] Tables are sorted by the engine; clicking a column header to re-sort is not built.
- [ ] No threshold control for Bursts in the app (`--threshold` exists in the CLI).
- [ ] Block lists by provider (ASN) are not possible: there is no offline list of an ASN's prefixes.
- [ ] Bots with neither a published list nor a documented reverse DNS domain stay unverifiable (DuckDuckGo, You.com, Exa, most SEO tools). DuckDuckGo's list did not respond when tried; Amazon's page is HTML, not JSON.
- [ ] Meta's ranges come over plain whois, which is unauthenticated (documented in the README).
- [ ] IP truncation option for exports (10).
- [ ] Abuse reports from the spec not built: distributed patterns, expensive targets, scanners as their own report (6.3.2 to 6.3.4). Bursts, status codes, top IPs, query strings and referrers cover much of the ground.
- [ ] Landing pages count `/feed/` and `.php` endpoints as pages on WordPress sites; tune `crates/core/data/sources.toml`.
- [ ] Fetching logs over SSH (9).
- [ ] A product brief for the Impeccable design skill (`/impeccable init`) and a scored design critique; only a polish pass was done.

## Done

### Engine

- [x] Combined-format parser, hand-written, with auto-detection (4.1)
- [x] Streaming ingest of plain and gzip files, one transaction per file (4)
- [x] De-duplication, grown-file resume, overlap policies `error` / `skip` / `allow` (4.3)
- [x] One DuckDB file per site, settings in a `meta` table, locking with `--wait-lock` (3.1)
- [x] Per-site time zone with local-time column, and changing it later (3.4)
- [x] Retention modes, prune, clear, compact, delete by ingested file (3.3)
- [x] Daily rollups: totals, sources, bots (3.2)
- [x] User-agent classification and traffic source rules, both in editable data files (5.3, 6.1)
- [x] Bot registry and verification against published IP ranges, with range history, a 30-day grace, extra ranges, and refusal of suspicious updates (5.1)
- [x] Nine operators with ranges: Google (common, special, user-triggered), Bing, OpenAI, Anthropic, Perplexity, Apple, Common Crawl, Meta (via RADB whois)
- [x] Forward-confirmed reverse DNS, cached, optional (5.1 step 3): Amazon, Yandex, Baidu, plus a second check for Google, Bing, Apple
- [x] Country and ASN lookup from DB-IP Lite or the user's MaxMind files (5.2)
- [x] Re-enrich: user agents, bot verification, geo, then rollups (5.4, partly; see gaps)
- [x] "Behind a caching CDN" site setting that adds a note to every report (4.2)

### Reports

- [x] Traffic: overview, sources, campaigns, landing pages, geography, AI referrals (6.1)
- [x] Bots: all bots, fake bots, bot IPs, AI crawlers, undeclared (6.2)
- [x] Abuse: bursts, status codes, top IPs, query strings, referrers (6.3, plus additions)
- [x] Requests drill-down, paged (6.5)
- [x] Block lists: plain, nginx `deny`, Apache `<RequireAll>`; verified bots and local addresses always left out (6.3.6)
- [x] Shared filters: dates, status class and exact code, path prefix, IP, source, country, ASN, bot, verified state, referrer, user agent
- [x] JSON envelope with named tables, paging, totals, coverage and notes (7.2)

### Command line

- [x] `site`, `ingest`, `report`, `reports`, `db`, `ranges`, `geo`, `reenrich`, `blocklist` (7.1)
- [x] `--json`, `--offline`, `--dns`, `--wait-lock`, exit code 75 when a site is locked

### Desktop app

- [x] Tauri 2 with Vue 3 and Nuxt UI: sites, ingest with drag and drop and progress, every report, storage, settings (8)
- [x] Filter bar with active-filter strip, clickable cells that filter every report, hourly or daily charts chosen automatically
- [x] Block-list selection with a preview before copying
- [x] Light, dark or system appearance
- [x] Single instance
- [x] Content security policy; icons bundled so the app makes no requests of its own
- [x] Windows installers: setup `.exe` (per user) and `.msi`, unsigned

### Project

- [x] Security audit and fixes (site-name validation, read-only report connections without external access, CSV formula guarding, prefix sanity checks)
- [x] Client-identifying strings removed from code, tests and history
- [x] Renamed to Logloupe; MIT licence; GitHub organisation and private repository; logloupe.com registered
- [x] CI on Windows, macOS and Linux: format, clippy, tests
- [x] Synthetic log generator with a burst, fake bots and a query-string loop, using only documentation address ranges for anything accused
- [x] Screenshots from the real app on synthetic data (`tools/screenshots.mjs`)
- [x] Product brief for the website session
