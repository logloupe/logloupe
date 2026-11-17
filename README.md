# Logloupe (`logloupe`)

Analyses web server access logs: traffic sources, bots, abuse and crawl problems. It checks crawlers against the IP ranges their operators publish and against reverse DNS, so a real Googlebot can be told from something pretending to be one, finds bursts of traffic and what caused them, and writes block lists for nginx and Apache. Everything stays on your machine.

One Rust engine (`crates/core`) with a command-line tool (`crates/cli`) and a desktop app (`crates/app`). The full design is in [spec.md](spec.md), and what is done and what is left is in [TODO.md](TODO.md).

Status: **early release.** Working today: ingest of plain and gzip combined-format logs (nginx, Apache, cPanel), de-duplication, traffic, bot and abuse reports, bot verification, country and network lookup, and block lists. Built since the last release: fetching logs from a server over SSH, and the SEO crawl reports (what verified search crawlers fetched, the errors and redirects they were served, crawl waste, robots.txt and sitemap fetches, Googlebot smartphone against desktop). Not built yet: custom log formats, and the reports that need a sitemap (uncrawled and orphan URLs). The desktop app is only built for Windows so far.

## Download and install

Releases are on the [releases page](https://github.com/logloupe/logloupe/releases).

- **Desktop app (Windows 10 and 11):** download `Logloupe_<version>_x64-setup.exe` and run it. It installs for the current user and needs no administrator rights.
- **Command-line tool (Windows, Linux, macOS on Apple silicon):** download the archive for your system, unpack it and put `logloupe` somewhere on your `PATH`.

The installer is not signed yet, so Windows SmartScreen says "Windows protected your PC" the first time. Choose **More info**, then **Run anyway**. Each release has a `SHA256SUMS.txt` to check the download against (`certutil -hashfile <file> SHA256`).

To update, run a newer installer over the old one; your data is kept. To remove the app, use **Settings > Apps > Installed apps > Logloupe > Uninstall**. Sites and settings live in `%APPDATA%\logloupe` and stay there unless you delete that folder too.

## Quick start

```
logloupe site add example --host example.com --host www.example.com --timezone Europe/London
logloupe ingest example /var/log/nginx/access.log*
logloupe reports
logloupe report example sources
logloupe report example overview --from 2026-11-01 --to 2026-11-14 --format json
logloupe db stats
```

- Data lives in the per-user data directory, or wherever `--data-dir` / `LOGLOUPE_DATA_DIR` points. Each site is one DuckDB file under `sites/`.
- `report --format json` writes exactly one JSON document to stdout (the envelope in spec 7.2). Other commands take `--json`.
- Ingesting a file twice is a no-op. A log that has grown since last time only adds its new lines.
- Exit codes: 0 success, 1 failure, 2 usage error, 75 site in use by another process (retry, or pass `--wait-lock <seconds>`).

## Fetching logs from a server

Not in a release yet; build from source to use it.

```
logloupe remote set example --host web1 --path '/var/log/nginx/access.log*'
logloupe remote list example        # what is on the server, and what a fetch would download
logloupe fetch example              # download new and changed files, ingest them, delete the copies
logloupe fetch example --days 30    # the first time, on a server with years of archives
```

- `--host` is a host alias from your SSH config, or `user@host`. Logloupe runs your computer's own `ssh` program, so it uses your config, keys and agent. It stores no keys or passwords and never asks for one: **`ssh <host>` must already work in a terminal without asking for anything** (host key accepted, key in the agent or without a passphrase).
- `--path` takes wildcards and can be repeated. Quote it so your own shell does not expand it. Common places: `/var/log/nginx/access.log*`; on cPanel, `~/logs/<domain>-*.gz` for the monthly archives; on ISPConfig, `~/log/*access.log*`.
- The account must be able to read the logs. On Debian and Ubuntu the nginx and Apache logs belong to the `adm` group, so a deploy user usually cannot: `remote list` then reports the files as unreadable. Fix it on the server as root with `usermod -aG adm <user>` (it takes effect at the next login).
- Each file is copied to a temporary file in the data directory, ingested like any local file, then deleted. A file that has not changed since the last fetch is not downloaded again. One that has grown is downloaded in full, but only its new lines are added.
- Nothing is written to the server. The only commands run there are `stat`, `test`, `echo` and `head`.
- A login script on the server that prints text for non-interactive logins breaks the download (it does the same to `scp`); Logloupe detects this and says so.

In the app this is on the Ingest screen, under "Fetch from a server". The design and the rules for what is accepted as a host or path are in [spec.md](spec.md) section 9.

## Desktop app

`crates/app` is a Tauri 2 app (Vue 3 + Nuxt UI) over the same engine. Its commands call the same functions as the CLI.

```
cd crates/app
pnpm install
pnpm build                                  # builds the frontend into dist/
cd ../..
cargo build --release --workspace           # target/release/logloupe-app(.exe)
```

Run `target/release/logloupe-app`. It uses the same data directory as the CLI, so sites created in one show up in the other (set `LOGLOUPE_DATA_DIR` to point it elsewhere). Only one process can write to a site at a time.

For frontend work, `pnpm dev` serves the UI at http://localhost:1420 with sample data in a plain browser, and `pnpm tauri dev -- --no-default-features` runs the real app against the dev server.

## Bot verification

```
logloupe ranges update          # fetch every operator's published IP lists
logloupe ranges status
logloupe reenrich example       # re-check stored rows against the current lists
logloupe report example fake-bots
```

Ingest refreshes lists older than a day unless `--offline` is given. The bot list and its sources are in `crates/core/data/bots.toml`; a copy at `<data-dir>/bots.toml` overrides it.

## Country and network (ASN) lookup

```
logloupe geo update             # download the free DB-IP Lite country and ASN databases (about 10 MB)
logloupe geo status
logloupe reenrich example       # fill in country and network for rows already ingested
logloupe report example top-ips --table by_asn
```

The app has the same under Settings. To use MaxMind instead, put your own `GeoLite2-Country.mmdb` and `GeoLite2-ASN.mmdb` in `<data-dir>/geo/`; they take priority over the DB-IP files. With no database installed, ingest still works and those columns stay empty.

IP geolocation by [DB-IP](https://db-ip.com), used under its Creative Commons Attribution licence.

## Security and privacy notes

- **Nothing is sent anywhere.** The only outbound requests are the bot IP lists, the DB-IP database download, reverse DNS lookups, SSH connections to a server you have set up for a site, and (later) sitemap fetches. Each is started by the user or can be turned off with `--offline` / "Work offline". The app bundles its icons and runs under a content security policy that blocks every other connection.
- **Fetching over SSH treats both ends as untrusted.** The host and the paths you enter are checked before they reach `ssh` or the server's shell: a host cannot start with `-` (which `ssh` would read as an option), and a path may hold only letters, digits, a few punctuation marks and the wildcards `* ? [ ]`. File names the server sends back are checked the same way and quoted, a download is cut off the moment it exceeds the size the server announced, and `ssh` runs in batch mode so it can never sit waiting for a password.
- **Access logs are personal data.** They contain IP addresses. Keep `samples/` and site databases out of version control and backups you do not control.
- **Log content is untrusted.** Paths, referrers and user agents are written by whoever sent the request. They are stored through DuckDB's appender, never spliced into SQL; filter values are escaped; report connections are read-only with DuckDB's external file and network access switched off. CSV exports prefix cells that start with `=`, `+`, `-` or `@` with an apostrophe so a spreadsheet does not run them as formulas. The CLI replaces control characters before printing.
- **Site names are validated** wherever they are used, so a name such as `../other` cannot make the tool open or delete a file outside the data directory.
- **Published IP lists are checked before use**: a fetched list is refused if it is too short, fails to parse, or contains a prefix broader than a /8 (IPv4) or /16 (IPv6). Meta's ranges come from a whois query, which is not encrypted or authenticated; someone able to tamper with that connection could get addresses wrongly marked as verified Meta bots (they could not get anything marked fake, and the breadth check limits how much they could add).
- **Dependency audits** (`cargo audit`, `pnpm audit`) on 2026-11-15 reported no known vulnerabilities in what ships. They flagged two advisories in Tauri's Linux GTK stack (`glib` unsoundness, `proc-macro-error` unmaintained) and one low-severity issue in `esbuild` that affects only the development server.

## Code signing policy

Releases are built by GitHub Actions from the tagged source in this repository ([release.yml](.github/workflows/release.yml)); nothing is built or altered by hand.

The Windows installer is **not signed yet**. The project applied to the SignPath Foundation programme for open source projects on 17 November 2026. Once the application is approved, releases will carry: free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org). Every signed release will be approved by hand by an approver listed below.

- Committers and reviewers: [robwent](https://github.com/robwent)
- Approvers: [robwent](https://github.com/robwent)

Privacy policy: this program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it. The requests it can make are listed under "Security and privacy notes" above; each one is started by the user or can be switched off.

## Building

Prerequisites: Rust stable, and a C++ compiler for the bundled DuckDB (on Windows, Visual Studio Build Tools with the "Desktop development with C++" workload).

```
cargo build --release -p logloupe        # binary at target/release/logloupe
cargo test --workspace --release
cargo clippy --workspace --all-targets --release -- -D warnings
```

The first build compiles DuckDB from source and takes 15–20 minutes. Use `--release` for tests as well, so they share that one DuckDB build instead of compiling a second (debug) copy.

## Checking and benchmarking

```
# Parse-only check of a plain-text log with the combined parser
cargo run --release -p logloupe-core --example parse_check -- access.log

# Deterministic synthetic log: browser/bot/tool mix, a fake Googlebot, one burst
cargo run --release -p logloupe-core --example gen_log -- --size 1GB --days 7 --out big.log
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
