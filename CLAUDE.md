# Logloupe

A web server access log analyser: a Rust engine (`crates/core`) with a command-line tool (`crates/cli`) and a Tauri desktop app (`crates/app`). It verifies bots against published IP ranges and reverse DNS, finds bursts and what caused them, and produces block lists.

## Start here

- **[TODO.md](TODO.md)**: what is done, what is next, what is waiting on the owner, and what has not been verified. Read it before starting work. **Update it in the same commit as the work**: tick what you finished and add anything new.
- **[spec.md](spec.md)**: the design. Section numbers in TODO.md refer to it. Change the spec when a decision changes.
- **[README.md](README.md)**: how to build and use it, security notes, measured performance.

## Rules that are easy to get wrong

- **All logic lives in `crates/core`.** A CLI command and an app command for the same feature call the same core function and return the same struct. The CLI and app only parse input and format output.
- **Build and test with `--release`**, always: `cargo test --workspace --release`. The bundled DuckDB takes 15 to 20 minutes to compile, and a debug build compiles a second copy. If `cargo` is not found, it is in `%USERPROFILE%\.cargo\bin`.
- **Before every commit**, run all three and commit only if each passes: `cargo fmt --all`, `cargo clippy --workspace --all-targets --release -- -D warnings`, `cargo test --workspace --release`. Do not chain the commit so that it runs after a failed check; one broken commit was pushed that way.
- **The app embeds `crates/app/dist` at compile time.** Run `pnpm build` in `crates/app` before building the app crate, or the build fails.
- **The app's `.exe` cannot be replaced while it is running.** Check for a running `logloupe-app` process before building, and ask the owner to close it.
- **Never put client data in the repository.** `samples/` holds real client logs and is git-ignored. Do not copy site names, URL paths, user agents, referrers or IP addresses from those logs into code, tests, fixtures, docs or screenshots. Use `example.com`, invented paths, and the documentation address ranges (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32). This has had to be cleaned up twice.
- **Log content is untrusted.** Values from logs go into the database through the appender, never into SQL text. Filter values are escaped with `report::quote`. Block-list entries are parsed and re-serialised, never passed through.
- **The app makes no network requests of its own.** The only outbound calls are ones the user starts (IP range lists, the geo database, reverse DNS), and each can be switched off. Keep it that way; the content security policy enforces it for the frontend.
- **Edit files with the Edit tool**, not with scripts that rewrite source through shell heredocs. That is how escaped newlines were turned into literal ones.

## Working with the running app

- The installed app and the CLI share one data directory (override with `LOGLOUPE_DATA_DIR`). Use a scratch directory for experiments so the owner's sites are not touched.
- To drive the real app for testing or screenshots, start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` and run `node tools/screenshots.mjs 9333 <output-folder>`. That script shows how to navigate, set filters, select rows and capture.
- Screenshots must come from synthetic data: `cargo run --release -p logloupe-core --example gen_log -- --size 150MB --days 7 --out demo.log`.

## The website

logloupe.com is built in a separate session in `C:\laragon\www\logloupe`. It works from `PRODUCT-BRIEF.md` in that folder. When something the site describes changes (a feature, an output format it imitates, release status), update the brief and tell that session. Nothing is released yet: no download, and the repository is private.
