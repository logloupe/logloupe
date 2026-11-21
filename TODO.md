# Logloupe: status and to-do

The single list of what is done and what is left. Update it in the same commit as the work: tick the item, move it if priorities change, and add anything new that comes up. The design behind each item is in [spec.md](spec.md); section numbers below refer to it.

Last updated: 2026-11-19. The history was rewritten on 2026-11-16, so commit hashes quoted anywhere before then no longer exist.

## Where things stand

- Engine, command-line tool and desktop app all work on Windows. CI passes on Windows, macOS and Linux (build and tests only; the app has never been opened on macOS or Linux).
- The owner uses the installed app on real client logs.
- The repository is `logloupe/logloupe` on GitHub, **public** since 2026-11-17. **There are no releases.** On 2026-11-21 the owner had all of them deleted (0.1.0 published, 0.2.0 and 0.3.0 drafts) with their tags, to start fresh. The next `v*` tag builds a new draft; the owner installs it and publishes it by hand. Release notes and the README must not say anything about signing applications or their outcome: only that the installer is not signed. The website at logloupe.com is built in a separate session; its brief says there is no download until the next release is published.
- The project lives in `C:\log-analyser`. It was moved off the USB drive `D:` on 2026-11-17 after that drive kept disconnecting; the old copy on `D:` is stale.
- 102 tests, all passing. `cargo clippy` is clean with warnings as errors.

## Next up, in order

- [ ] **Data-centre browsers in Undeclared** (6.2.5). Now that ASN lookup exists, flag browser user agents coming from hosting providers at volume. Needs a way to tell hosting ASNs from access ISPs; decide that first.
- [ ] **Custom log formats** (4.1): "combined + extras" (request time, vhost, forwarded IP) and user-supplied nginx `log_format` / Apache `LogFormat` strings, set per site. **Blocked on the owner** supplying a sample log in a non-default format. (A cPanel/WHM monthly archive has been supplied: it is plain combined format with IPv4 and IPv6 clients, so cPanel needs nothing custom.)
- [ ] **Proxy / CDN detection** (4.2): detect logs that record Cloudflare's address instead of the visitor's, and use a forwarded-IP field when the format has one. Depends on custom formats.
- [ ] **Known URLs** (6.4): fetch a sitemap or import a URL list, then "uncrawled" and "orphan" reports (6.4 items 6 and 7, the only two of the SEO reports not built). The `known_urls` table exists and is unused. The orphan report can reuse the crawl-waste categories to leave out assets and junk. Finding the sitemap without being told where it is (owner's idea, 2026-11-17): read the site's `robots.txt` for `Sitemap:` lines first, then try the usual locations (`/sitemap.xml`, `/sitemap_index.xml`, `/wp-sitemap.xml` and the like). Still a fetch the user starts, and one that can be switched off, like every other outbound call.
- [ ] **Desktop app for Linux in releases** (13): the release workflow builds the Windows installer and the command-line tool for Windows, Linux x64 and macOS arm64. The app has never been opened on Linux, so it is not shipped there. **macOS is set aside** (owner's decision, 2026-11-18: Windows and Linux for the time being): no macOS app, no Apple Developer account, no macOS signing. The macOS arm64 command-line build is still produced by the workflow, untested by hand.
- [ ] **Code signing with SignPath Foundation** (applied 2026-11-17, declined; apply again later, see "Waiting on the owner"): free signing for open source projects. Their terms require the project to be already released in the form to be signed, and the application form (signpath.org/apply) has a required "Reputation" field: links showing the project is widely used or trusted (download statistics, GitHub insights, coverage, discussions), plus a project name that a web search clearly identifies. So: release unsigned, build up some evidence of use, then apply. Also needed then: the attribution line "Free code signing provided by SignPath.io, certificate by SignPath Foundation" in the README's code signing policy and on the download page, multi-factor authentication on GitHub and SignPath, and a signing step in the release workflow that the owner approves per release. The form asks for name and email for the SignPath account, and has a reCAPTCHA, so the owner submits it.
- [ ] **Faster release builds**: the first release run took 1 h 24 min. On Windows the installer step took 44 minutes and the command-line step after it another 37, which means DuckDB was compiled twice; find out why the second `cargo build -p logloupe` does not reuse the first build (different features or environment under `tauri build`) and build both in one pass. The build cache is also not shared between tags.
- [ ] **Updater**: Tauri's updater plugin, checking GitHub releases. Needs the repository public and an update signing key pair (separate from code signing; the private key is the owner's to hold). Decided: a "Check for updates" button in Settings and no automatic check, because the app promises to make no requests the user did not start.
- [ ] **Server version**: an HTTP server exposing the same operations, serving the same frontend, with login (password first, then OpenID Connect) and a read-only API with tokens. Reads logs only from configured folders. The spec lists this as a non-goal and needs changing when work starts.
- [ ] **MCP over HTTP** (7.3): the stdio server exists (`logloupe mcp`, see Done). What is left is a transport a client can reach by URL, which connectors added through claude.ai need: streamable HTTP with a bearer token, as part of the server version, since that is where login, tokens and TLS guidance belong. It hands each request body to the same `mcp::Server::handle`, so there are no tools to write again. Until then a server's logs are reached with `ssh host logloupe mcp`, and AIdata can also call the `logloupe` binary and read its JSON.

## Waiting on the owner

- [ ] Install the 0.3.0 draft on the second computer, try connecting an assistant from the Settings card ("Use with an AI assistant"), then publish it. The 0.2.0 draft can be deleted or published first; 0.3.0 replaces it either way.
- [ ] Publish the 0.2.0 draft release (or skip it for 0.3.0). The owner installed it on a second computer and used it on 2026-11-18 (the notes behind sorting, the time zone picker and MCP came from that session); it was still a draft on GitHub that day. Once published, the README status line ("Built since the last release") and the website's brief need bringing up to date.
- [ ] Sample log in a custom nginx `log_format`. Most of the owner's servers use the standard nginx format. One, managed through WHM, may not; its Apache logs are easy to get, its nginx ones not yet. What is needed is the `log_format` line from that server's nginx configuration (`grep -rn log_format /etc/nginx/`) and a few lines of the log it describes, kept in `samples/`.
- [ ] Try the SSH fetch on a cPanel account if one is to hand.
- [ ] SignPath declined the application of 2026-11-17 (reported by the owner on 2026-11-18) and said to apply again once the project has more traction. Releases stay unsigned until then. When re-applying: download figures and other evidence of use are what was missing. If approved: add the signing step to the release workflow, change the code signing policy to the present tense, and tell the website.
- [ ] The website folder and its product brief have not been re-read for client-identifying strings. The site has not been touched since it was first built, so it describes none of the features added since: SSH fetch, the SEO reports, sorting, the time zone picker, MCP.
- [ ] Confirm in the installed app: the second launch brings the window to the front (single-instance), and data moved across after the rename.

## Not yet verified

Things built and tested in the engine or from the command line, but not exercised by hand in the app:

- [ ] **SSH fetch against a real server.** The tests use a stand-in server, plus one that runs the real listing and download scripts through a local `sh`. On 2026-11-17 the command-line tool was run against one of the owner's own Ubuntu servers with the Windows OpenSSH client: files the account could not read were reported as such, and once the account was in the `adm` group 15 nginx logs (13 gzip, 2 plain, 109 rows) were fetched and ingested, a second fetch downloaded nothing, and no temporary file was left. Those logs are tiny, so the same day it was run against a busier server (ISPConfig layout, logs under `~/log/`): 11 files, 53 MB, about 701,000 rows, fetched and ingested in 36 seconds with no bad lines; a second fetch skipped the 10 unchanged files and resumed the live log, adding only its 28 new lines. Not yet seen: a cPanel jailed shell, a server with BSD `stat`, a server whose login script prints text, a multi-gigabyte log, cancelling mid-download.
- [ ] The owner ran a fetch from the "Fetch from a server" card on 2026-11-17 and it worked. The bot IP range status line and the "Do not refresh bot IP ranges first" switch were added after that and have not been looked at in the app.
- [ ] **SEO crawl reports on real logs.** Built and tested on 2026-11-17 against a hand-written fixture and a generated 30 MB log, from the command line and by opening all six screens in the app. The owner tried the app build the same day and was happy with it; changes will follow from use. Not confirmed on a real site's logs, so possibly unseen: how the waste patterns read on real URLs, Googlebot Image / Video / News user agents, a site with assets crawled, a large site's speed (each screen runs two to four grouped queries over the crawler rows), and the "slowest" table, which needs a log with response times.
- [ ] The "Bot address" (verified state) choice in the app's filter bar is new and was not clicked through; the SEO screens were only opened with no filters set.
- [ ] **`logloupe mcp` with an assistant.** Tested on 2026-11-18 through its own tests, by piping a whole session into the built program on generated data (its block list matched the command line's), and with the MCP Inspector's command-line client, which connected, listed the tools with and without `--allow-fetch`, ran a report and got the "no server set up" refusal from `fetch_logs`. Never connected to an assistant (Claude Code, Claude Desktop or another), so unseen: how one copes with the size of a report, running it through `ssh`, and what a client does when a site is locked. That run also showed that a client may not pass `LOGLOUPE_DATA_DIR` on, so the README says to use `--data-dir`.
- [ ] **The Settings card's instructions, followed by a person.** On 2026-11-18 the card was opened in the real app on generated data and its text read back, and the MCP Inspector connected to `logloupe-app mcp` (tools listed, a report run while the window was open). Nobody has pasted the text into Claude Desktop or run the `claude mcp add` line, and the wording about where Claude Desktop keeps its settings file ("Settings, Developer, Edit Config") was written from memory. The owner will try it from the next release build; the path shown will then be the installed one.
- [ ] **`fetch_logs` against a real server.** Only tested with a stand-in server in the engine's tests. Not seen: a real fetch started by an assistant, how long a client waits before giving up on one, and whether clients ask before running a tool marked as not read-only.
- [ ] Column sorting and the time zone picker (2026-11-18) were driven in the real app on generated data: the three-click cycle on the All bots table, and searching for, choosing and saving a zone when editing a site. Not looked at: sorting on other screens, sorting then paging or exporting by hand, the picker in the "Add site" form, dark mode.

- [ ] Dark mode on every screen (the chart is the likeliest problem).
- [ ] CSV export, the Copy button on block lists, "View requests", the Geography and Status codes pages, the Settings downloads.
- [ ] Reverse DNS running from inside the app (only run from the CLI so far).
- [ ] The app on macOS and Linux.
- [ ] Performance re-measured since bot verification, geo lookup and reverse DNS were added to ingest. Last figures (1 GB in about 25 s, 681 MB peak) predate them. Also unmeasured: gzip input, a 10 GB file, a 30-day site.

## Smaller gaps

- [ ] "View requests" from an SEO screen shows every request matching the filter bar, not only the verified search crawlers the screen counted. Click a bot first, or set "Bot address" to Verified.
- [ ] SEO reports count verified crawlers or one other verified state, never verified and unverifiable together. Yandex and Baidu are only verifiable with reverse DNS on, and DuckDuckGo not at all, so they are left out by default (the notes give the number).
- [ ] The generated demo log has no asset or image-crawler requests from bots, so the crawl-waste "asset" category is empty in screenshots.
- [ ] Rollup fallback for pruned days exists only for the Overview report (3.5). Sources, geography and bots report on remaining raw rows and say so.
- [ ] `reenrich` takes no date range and does not recompute `source_type` after hostnames change (5.4). Re-ingest with `--force` is the workaround.
- [ ] Arbitrary CIDR filters: only an exact address, an IPv4 /24 or an IPv6 /48 work (3.2).
- [ ] MCP: `fetch_logs` reports no progress and cannot be cancelled; `list_sites` does not say which sites have a server set up, so an assistant finds out by trying.
- [ ] The time zone picker opens at the top of its list, not at the zone currently chosen.
- [ ] No threshold control for Bursts in the app (`--threshold` exists in the CLI).
- [ ] Block lists by provider (ASN) are not possible: there is no offline list of an ASN's prefixes.
- [ ] Bots with neither a published list nor a documented reverse DNS domain stay unverifiable (DuckDuckGo, You.com, Exa, most SEO tools). DuckDuckGo's list did not respond when tried; Amazon's page is HTML, not JSON.
- [ ] Meta's ranges come over plain whois, which is unauthenticated (documented in the README).
- [ ] IP truncation option for exports (10).
- [ ] Abuse reports from the spec not built: distributed patterns, expensive targets, scanners as their own report (6.3.2 to 6.3.4). Bursts, status codes, top IPs, query strings and referrers cover much of the ground.
- [ ] Landing pages count `/feed/` and `.php` endpoints as pages on WordPress sites; tune `crates/core/data/sources.toml`.
- [ ] SSH fetch downloads a grown live log in full each time, though only its new lines are ingested (9.1). Fetching just the new tail needs ingest to read from a stream.
- [ ] SSH fetch: one server per site (9.3); nothing scheduled, a fetch runs when asked; the app has no "fetch again and replace" switch (`fetch --force` in the CLI).
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
- [x] Fetching logs over SSH (9): download to a temporary file, then ingest as usual. One remote per site in `meta`; listing with new / changed / fetched / old states; unchanged files are not downloaded again; hosts, path patterns and the file names a server returns are all validated; download size is capped at what the listing announced. `crates/core/src/remote.rs`

### Reports

- [x] Traffic: overview, sources, campaigns, landing pages, geography, AI referrals (6.1)
- [x] Bots: all bots, fake bots, bot IPs, AI crawlers, undeclared (6.2)
- [x] Abuse: bursts, status codes, top IPs, query strings, referrers (6.3, plus additions)
- [x] SEO: crawl overview, crawled URLs (with least crawled, largest, slowest), crawl errors (errors, redirects, inconsistent), crawl waste, robots and sitemap fetches, Googlebot smartphone against desktop (6.4, all but the two that need known URLs). Verified search crawlers by default; the notes count what was left out. `crates/core/src/report/crawl.rs`
- [x] Requests drill-down, paged (6.5)
- [x] Block lists: plain, nginx `deny`, Apache `<RequireAll>`; verified bots and local addresses always left out (6.3.6)
- [x] Shared filters: dates, status class and exact code, path prefix, IP, source, country, ASN, bot, verified state (in the app's filter bar too, as "Bot address"), referrer, user agent
- [x] JSON envelope with named tables, paging, totals, coverage and notes (7.2)
- [x] Sorting by any column, done by the engine before paging (`sort` and `descending` in the report parameters; `--sort COLUMN [--desc]`)

### Command line

- [x] `site`, `ingest`, `report`, `reports`, `db`, `ranges`, `geo`, `reenrich`, `blocklist` (7.1)
- [x] `--json`, `--offline`, `--dns`, `--wait-lock`, exit code 75 when a site is locked
- [x] `remote set|show|remove|list` and `fetch` (9.7)
- [x] `site timezones [search]`
- [x] `mcp`: a read-only Model Context Protocol server on standard input and output, for AI assistants (7.3). Four tools: `list_sites`, `list_reports`, `run_report`, `block_list`. The protocol and tools are in `crates/core/src/mcp.rs`; the command is only the transport
- [x] `mcp --allow-fetch`: adds `fetch_logs`, so the assistant can pull the latest logs for a site that has a server set up. Off by default; takes only a site and a number of days; never replaces stored rows

### Desktop app

- [x] Tauri 2 with Vue 3 and Nuxt UI: sites, ingest with drag and drop and progress, every report, storage, settings (8)
- [x] Filter bar with active-filter strip, clickable cells that filter every report, hourly or daily charts chosen automatically
- [x] Block-list selection with a preview before copying
- [x] Sortable column headers on every report table: first click sorts (numbers largest first, text A to Z), second reverses, third returns to the usual order. Paging and CSV export keep the chosen order
- [x] Searchable time zone picker when adding or editing a site, filled from the engine's own list of names and showing each zone's offset from UTC (`logloupe site timezones [search]` on the command line)
- [x] "Fetch from a server" on the Ingest screen: set-up form, "Check server" listing, "Fetch new files" with the same progress, cancel and results as a local ingest
- [x] Ingest and fetch both say what happened to the bot IP ranges first: being updated, updated, already up to date, or not refreshed
- [x] "Use with an AI assistant" in Settings: the text to paste into Claude Desktop or Claude Code, built from where the app is installed, with a switch for letting the assistant fetch logs. The app serves MCP itself (`logloupe-app mcp`), so the command-line tool is not needed
- [x] Light, dark or system appearance
- [x] Single instance
- [x] Content security policy; icons bundled so the app makes no requests of its own
- [x] Windows installers: setup `.exe` (per user) and `.msi`, unsigned

### Project

- [x] Security audit and fixes (site-name validation, read-only report connections without external access, CSV formula guarding, prefix sanity checks)
- [x] Client-identifying strings removed from code, tests and history
- [x] Independent re-read against the real logs (every committed line compared with the addresses, hostnames, paths, user agents and account names in `samples/`). It found one log's size, row count and timestamps in the browser preview data, and a real address in an early version of the log generator; both were replaced throughout the history
- [x] The commit that did not compile was folded into its fix, so every commit in the history is one that was meant to build
- [x] Renamed to Logloupe; MIT licence; GitHub organisation and repository (public since 2026-11-17); logloupe.com registered
- [x] CI on Windows, macOS and Linux: format, clippy, tests
- [x] Release workflow: a `v*` tag builds the Windows setup program and the command-line tool for three systems, with checksums, into a draft GitHub release (13)
- [x] Synthetic log generator with a burst, fake bots, a query-string loop and both Googlebot crawlers (smartphone and desktop), using only documentation address ranges for anything accused
- [x] Screenshots from the real app on synthetic data (`tools/screenshots.mjs`)
- [x] Product brief for the website session
