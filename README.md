# Log Analyser (`loga`)

Analyses web server access logs: traffic sources, bots, abuse and crawl problems. One Rust engine (`crates/core`) with a command-line front end (`crates/cli`); a desktop app comes later. The full design is in [spec.md](spec.md).

Status: **Phase 1 in progress.** Working today: site management, ingest of plain and gzip combined-format logs, de-duplication, daily rollups, and the traffic reports (overview, sources, campaigns, landing pages, AI referrals).

## Quick start

```
loga site add example --host example.com --host www.example.com --timezone Europe/London
loga ingest example /var/log/nginx/access.log*
loga reports
loga report example sources
loga report example overview --from 2026-11-01 --to 2026-11-14 --format json
loga db stats
```

- Data lives in the per-user data directory, or wherever `--data-dir` / `LOGA_DATA_DIR` points. Each site is one DuckDB file under `sites/`.
- `report --format json` writes exactly one JSON document to stdout (the envelope in spec 7.2). Other commands take `--json`.
- Ingesting a file twice is a no-op. A log that has grown since last time only adds its new lines.
- Exit codes: 0 success, 1 failure, 2 usage error, 75 site in use by another process (retry, or pass `--wait-lock <seconds>`).

## Desktop app

`crates/app` is a Tauri 2 app (Vue 3 + Nuxt UI) over the same engine. Its commands call the same functions as the CLI.

```
cd crates/app
pnpm install
pnpm build                                  # builds the frontend into dist/
cd ../..
cargo build --release --workspace           # target/release/loga-app(.exe)
```

Run `target/release/loga-app`. It uses the same data directory as the CLI, so sites created in one show up in the other (set `LOGA_DATA_DIR` to point it elsewhere). Only one process can write to a site at a time.

For frontend work, `pnpm dev` serves the UI at http://localhost:1420 with sample data in a plain browser, and `pnpm tauri dev -- --no-default-features` runs the real app against the dev server.

## Bot verification

```
loga ranges update          # fetch every operator's published IP lists
loga ranges status
loga reenrich example       # re-check stored rows against the current lists
loga report example fake-bots
```

Ingest refreshes lists older than a day unless `--offline` is given. The bot list and its sources are in `crates/core/data/bots.toml`; a copy at `<data-dir>/bots.toml` overrides it.

## Country and network (ASN) lookup

```
loga geo update             # download the free DB-IP Lite country and ASN databases (about 10 MB)
loga geo status
loga reenrich example       # fill in country and network for rows already ingested
loga report example top-ips --table by_asn
```

The app has the same under Settings. To use MaxMind instead, put your own `GeoLite2-Country.mmdb` and `GeoLite2-ASN.mmdb` in `<data-dir>/geo/`; they take priority over the DB-IP files. With no database installed, ingest still works and those columns stay empty.

IP geolocation by [DB-IP](https://db-ip.com), used under its Creative Commons Attribution licence.

## Security and privacy notes

- **Nothing is sent anywhere.** The only outbound requests are the bot IP lists, the DB-IP database download, and (later) sitemap fetches. Each is started by the user or can be turned off with `--offline` / "Work offline". The app bundles its icons and runs under a content security policy that blocks every other connection.
- **Access logs are personal data.** They contain IP addresses. Keep `samples/` and site databases out of version control and backups you do not control.
- **Log content is untrusted.** Paths, referrers and user agents are written by whoever sent the request. They are stored through DuckDB's appender, never spliced into SQL; filter values are escaped; report connections are read-only with DuckDB's external file and network access switched off. CSV exports prefix cells that start with `=`, `+`, `-` or `@` with an apostrophe so a spreadsheet does not run them as formulas. The CLI replaces control characters before printing.
- **Site names are validated** wherever they are used, so a name such as `../other` cannot make the tool open or delete a file outside the data directory.
- **Published IP lists are checked before use**: a fetched list is refused if it is too short, fails to parse, or contains a prefix broader than a /8 (IPv4) or /16 (IPv6). Meta's ranges come from a whois query, which is not encrypted or authenticated; someone able to tamper with that connection could get addresses wrongly marked as verified Meta bots (they could not get anything marked fake, and the breadth check limits how much they could add).
- **Dependency audits** (`cargo audit`, `pnpm audit`) on 2026-11-15 reported no known vulnerabilities in what ships. They flagged two advisories in Tauri's Linux GTK stack (`glib` unsoundness, `proc-macro-error` unmaintained) and one low-severity issue in `esbuild` that affects only the development server.

## Building

Prerequisites: Rust stable, and a C++ compiler for the bundled DuckDB (on Windows, Visual Studio Build Tools with the "Desktop development with C++" workload).

```
cargo build --release -p loga        # binary at target/release/loga
cargo test --workspace --release
cargo clippy --workspace --all-targets --release -- -D warnings
```

The first build compiles DuckDB from source and takes 15–20 minutes. Use `--release` for tests as well, so they share that one DuckDB build instead of compiling a second (debug) copy.

## Checking and benchmarking

```
# Parse-only check of a plain-text log with the combined parser
cargo run --release -p loga-core --example parse_check -- access.log

# Deterministic synthetic log: browser/bot/tool mix, a fake Googlebot, one burst
cargo run --release -p loga-core --example gen_log -- --size 1GB --days 7 --out big.log
```

Real logs used for local checking go in `samples/`, which is git-ignored. Never commit client logs.

## Measured performance

Measured on the development machine (Windows 11, release build, default 512 MB DuckDB memory limit) with a 1 GB synthetic log of 4,582,725 lines from `gen_log --size 1GB --days 7`.

| Measure | Phase 1 |
|---|---|
| Ingest, 1 GB plain log | 22.6–25.6 s (42–47 MB/s, about 190,000 rows/s) |
| Peak resident memory during that ingest | 681 MB |
| Database size | 220–237 MB (about 21% of the log) |
| Re-ingest of the same file (no-op) | 0.35 s |
| `overview` over all 4.6 M rows | 0.80 s |
| `sources` | 0.17 s |
| `campaigns` | 0.20 s |
| `landing-pages` (session window) | 0.72 s |
| `ai-referrals` | 0.18 s |
| Parse only, real nginx logs | about 200 MB/s on one thread |

Not yet measured: gzip input, a 10 GB file (to confirm memory stays flat), and a 30-day, 1 M requests/day site.
