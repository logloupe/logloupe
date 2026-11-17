//! Fetching logs from a server over SSH (spec 9).
//!
//! Each remote file is downloaded to a temporary file, ingested like any
//! local file, and deleted. The system `ssh` client does the connecting, so
//! no keys or passwords are handled here. The settings and everything the
//! server sends back are treated as untrusted.

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use duckdb::params;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingest::{self, FileReport, IngestOptions, Origin};
use crate::store::{Mode, Retention, Store};

const META_REMOTE: &str = "remote";
const META_SEEN: &str = "remote_seen";

const MAX_PATTERNS: usize = 20;
const MAX_PATTERN_LEN: usize = 500;
const MAX_NAME_LEN: usize = 4096;
const MAX_FILES: usize = 10_000;
const MAX_LISTING_BYTES: u64 = 4 << 20;
const LIST_BEGIN: &str = "LOGLOUPE-LIST-BEGIN";
const LIST_END: &str = "LOGLOUPE-LIST-END";
/// Download progress is reported at most once per this many bytes.
const PROGRESS_EVERY_BYTES: u64 = 1 << 20;
/// Temporary files older than this were left by a crash.
const STALE_TEMP: Duration = Duration::from_secs(24 * 60 * 60);

/// Where a site's logs are fetched from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    /// An SSH host alias from `~/.ssh/config`, or `user@host`.
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    /// Path patterns on the server, expanded by its shell.
    pub paths: Vec<String>,
}

impl Remote {
    /// Trims the values and drops empty and repeated patterns.
    fn cleaned(&self) -> Remote {
        let mut paths: Vec<String> = Vec::new();
        for path in self.paths.iter().map(|p| p.trim()) {
            if !path.is_empty() && !paths.iter().any(|p| p == path) {
                paths.push(path.to_owned());
            }
        }
        Remote {
            host: self.host.trim().to_owned(),
            port: self.port,
            paths,
        }
    }

    /// Refuses anything that could be read as an `ssh` option or as shell
    /// syntax on the server.
    pub fn validate(&self) -> Result<()> {
        validate_host(&self.host)?;
        if self.port == Some(0) {
            return Err(Error::invalid("the port must be between 1 and 65535"));
        }
        if self.paths.is_empty() {
            return Err(Error::invalid(
                "give at least one path on the server, for example /var/log/nginx/access.log*",
            ));
        }
        if self.paths.len() > MAX_PATTERNS {
            return Err(Error::invalid(format!(
                "too many paths: at most {MAX_PATTERNS}"
            )));
        }
        self.paths.iter().try_for_each(|p| validate_pattern(p))
    }

    /// The value recorded in `ingested_files.source`.
    pub fn source(&self) -> String {
        format!("ssh:{}", self.host)
    }
}

fn validate_host(host: &str) -> Result<()> {
    let bad =
        |why: &str| Error::invalid(format!("invalid server '{}': {why}", printable(host, 80)));
    if host.is_empty() || host.len() > 255 {
        return Err(bad(
            "give an SSH host alias or user@host, for example web1 or deploy@example.com",
        ));
    }
    if !host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
    {
        return Err(bad(
            "use only letters, digits, '.', '_', '-' and one '@'. Put anything else, such as a jump host, in your SSH config and use the alias",
        ));
    }
    let (user, name) = match host.split_once('@') {
        Some((user, name)) => (Some(user), name),
        None => (None, host),
    };
    let starts_alnum = |s: &str| s.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric());
    if name.contains('@') || !starts_alnum(name) || user.is_some_and(|u| !starts_alnum(u)) {
        return Err(bad(
            "the user and the host must each start with a letter or digit",
        ));
    }
    Ok(())
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'.' | b'_' | b'-' | b'/' | b'+' | b',' | b'=' | b'@' | b'%' | b':'
        )
}

fn validate_pattern(pattern: &str) -> Result<()> {
    let bad =
        |why: &str| Error::invalid(format!("invalid path '{}': {why}", printable(pattern, 120)));
    if pattern.len() > MAX_PATTERN_LEN {
        return Err(bad("too long"));
    }
    let rest = match pattern.strip_prefix("~/") {
        Some(rest) => rest,
        None if pattern.starts_with('/') => pattern,
        None => return Err(bad("it must start with / or ~/")),
    };
    if !rest
        .bytes()
        .all(|b| is_name_byte(b) || matches!(b, b'*' | b'?' | b'[' | b']'))
    {
        return Err(bad(
            "use only letters, digits, . _ - / + , = @ % : and the wildcards * ? [ ]. Spaces and quotes are not supported",
        ));
    }
    Ok(())
}

/// A file name the server reported: absolute and free of anything a shell
/// would interpret.
fn valid_name(name: &str) -> bool {
    name.starts_with('/') && name.len() <= MAX_NAME_LEN && name.bytes().all(is_name_byte)
}

/// Text that may have come from the server or the user, made safe to show.
fn printable(text: &str, max_chars: usize) -> String {
    text.chars()
        .take(max_chars)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn remote_error(host: &str, message: impl Into<String>) -> Error {
    Error::Remote {
        host: host.to_owned(),
        message: message.into(),
    }
}

/// The site's remote, if one is set.
pub fn get(store: &Store) -> Result<Option<Remote>> {
    match store.meta(META_REMOTE)? {
        None => Ok(None),
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| Error::invalid(format!("corrupt 'remote' setting: {e}"))),
    }
}

/// Validates and stores the site's remote. Returns it as stored.
pub fn set(store: &Store, remote: &Remote) -> Result<Remote> {
    need_write(store)?;
    let remote = remote.cleaned();
    remote.validate()?;
    // What was fetched from one server says nothing about another.
    if get(store)
        .ok()
        .flatten()
        .is_some_and(|old| old.host != remote.host)
    {
        delete_meta(store, META_SEEN)?;
    }
    store.set_meta(
        META_REMOTE,
        &serde_json::to_string(&remote).expect("remotes serialise"),
    )?;
    Ok(remote)
}

/// Forgets the site's remote. Files already fetched stay ingested.
/// Returns false when there was none.
pub fn remove(store: &Store) -> Result<bool> {
    need_write(store)?;
    let existed = store.meta(META_REMOTE)?.is_some();
    delete_meta(store, META_REMOTE)?;
    delete_meta(store, META_SEEN)?;
    Ok(existed)
}

fn need_write(store: &Store) -> Result<()> {
    if store.mode() == Mode::ReadWrite {
        Ok(())
    } else {
        Err(Error::invalid("this needs the site opened read-write"))
    }
}

fn delete_meta(store: &Store, key: &str) -> Result<()> {
    store
        .conn
        .execute("DELETE FROM meta WHERE key = ?", params![key])?;
    Ok(())
}

fn require(store: &Store) -> Result<Remote> {
    let remote = get(store)?.ok_or_else(|| {
        Error::invalid(format!(
            "site '{}' has no server to fetch from yet",
            store.slug()
        ))
    })?;
    // Stored values are checked again: the database file could have been edited.
    remote.validate()?;
    Ok(remote)
}

/// What was fetched last time, per remote path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    size: u64,
    mtime: i64,
    /// The `ingested_files` row it became; `None` for an empty file.
    file_id: Option<i32>,
}

type SeenMap = BTreeMap<String, Seen>;

fn load_seen(store: &Store) -> Result<SeenMap> {
    // A record that cannot be read only costs a repeat download.
    Ok(store
        .meta(META_SEEN)?
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

fn save_seen(store: &Store, seen: &SeenMap) -> Result<()> {
    store.set_meta(
        META_SEEN,
        &serde_json::to_string(seen).expect("maps serialise"),
    )
}

fn file_ids(store: &Store) -> Result<HashSet<i32>> {
    let mut stmt = store.conn.prepare("SELECT file_id FROM ingested_files")?;
    let ids = stmt.query_map([], |row| row.get::<_, i32>(0))?;
    Ok(ids.collect::<duckdb::Result<_>>()?)
}

/// How the bytes get here. `Ssh` is the real one; tests use a stand-in.
pub trait Transport {
    /// Runs the listing and returns its raw output, which is untrusted.
    fn list(&self, remote: &Remote) -> Result<String>;

    /// Writes the first `size` bytes of `path` to `dest` and returns how many
    /// arrived. `path` has passed `valid_name`.
    fn download(
        &self,
        remote: &Remote,
        path: &str,
        size: u64,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
        cancel: &AtomicBool,
    ) -> Result<u64>;
}

/// The system `ssh` client.
#[derive(Debug, Default)]
pub struct Ssh {
    /// Run the scripts with a local `sh` instead, to test them.
    #[cfg(test)]
    local_shell: bool,
}

impl Ssh {
    pub fn new() -> Self {
        Ssh::default()
    }

    fn command(&self, remote: &Remote) -> Command {
        #[cfg(test)]
        if self.local_shell {
            return Command::new("sh");
        }
        let mut command = Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "Compression=yes",
            "-o",
            "LogLevel=ERROR",
        ]);
        if let Some(port) = remote.port {
            command.arg("-p").arg(port.to_string());
        }
        // The script arrives on standard input, so the account's login shell
        // only ever has to run the word `sh`.
        command.arg("--").arg(&remote.host).arg("sh");
        command
    }

    /// Starts `sh` on the server and feeds it `script`.
    fn run(&self, remote: &Remote, script: &str) -> Result<Running> {
        remote.validate()?;
        let mut command = self.command(remote);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up beside the app.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                remote_error(
                    &remote.host,
                    "the ssh program was not found. Install the OpenSSH client and make sure `ssh` is on the PATH",
                )
            } else {
                Error::Io(e)
            }
        })?;
        let stderr = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                let mut buf = [0u8; 4096];
                // Keep the start, but read to the end so `ssh` never blocks.
                while let Ok(n) = stderr.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if kept.len() < 8192 {
                        kept.extend_from_slice(&buf[..n]);
                    }
                }
                String::from_utf8_lossy(&kept).into_owned()
            })
        });
        let stdout = child.stdout.take();
        let mut running = Running {
            child,
            stdout,
            stderr,
            host: remote.host.clone(),
            done: false,
        };
        if let Some(mut stdin) = running.child.stdin.take() {
            // When login fails the pipe closes early; the exit status and
            // standard error say why, so a failed write is not reported here.
            let _ = stdin.write_all(script.as_bytes());
        }
        Ok(running)
    }
}

/// A running `ssh`. Killed on drop unless it was waited for.
struct Running {
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<String>>,
    host: String,
    done: bool,
}

impl Running {
    /// Waits for the process, once its output has been read to the end.
    fn finish(mut self) -> Result<()> {
        self.stdout = None;
        let status = self.child.wait();
        self.done = true;
        let said = self
            .stderr
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        let status = status?;
        if status.success() {
            return Ok(());
        }
        let mut message = printable(said.trim(), 500).trim_end_matches('.').to_owned();
        if message.is_empty() {
            message = match status.code() {
                Some(code) => format!("ssh exited with status {code}"),
                None => "ssh was stopped".to_owned(),
            };
        }
        // 255 is the client's own failure: no connection or no login.
        if status.code() == Some(255) {
            message.push_str(&format!(
                ". Check that `ssh {}` works in a terminal without asking for a password, a passphrase or to confirm the host key",
                self.host
            ));
        }
        Err(remote_error(&self.host, message))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The listing script: between two markers, a `P <n>` line per pattern, then
/// `F <size> <mtime> <path>` per readable regular file and `U <path>` per
/// unreadable one. Patterns are unquoted so the shell expands them; that is
/// safe only because `validate_pattern` allows nothing else a shell acts on.
fn list_script(remote: &Remote) -> String {
    let mut script = String::from(
        "if stat -c %s / >/dev/null 2>&1; then\n\
         \x20 st() { stat -L -c 'F %s %Y %n' -- \"$1\"; }\n\
         else\n\
         \x20 st() { stat -L -f 'F %z %m %N' -- \"$1\"; }\n\
         fi\n",
    );
    script.push_str(&format!("echo {LIST_BEGIN}\n"));
    for (index, pattern) in remote.paths.iter().enumerate() {
        script.push_str(&format!(
            "echo 'P {index}'\n\
             for f in {pattern}; do\n\
             \x20 [ -f \"$f\" ] || continue\n\
             \x20 if [ -r \"$f\" ]; then st \"$f\"; else printf 'U %s\\n' \"$f\"; fi\n\
             done\n"
        ));
    }
    script.push_str(&format!("echo {LIST_END}\n"));
    script
}

/// The download script. Taking exactly the listed size means a log that is
/// still being written cannot give a torn result.
fn download_script(path: &str, size: u64) -> Result<String> {
    if !valid_name(path) {
        return Err(Error::invalid(format!(
            "refusing to fetch a file with an unusual name: {}",
            printable(path, 120)
        )));
    }
    Ok(format!("head -c {size} -- '{path}'\n"))
}

impl Transport for Ssh {
    fn list(&self, remote: &Remote) -> Result<String> {
        let mut running = self.run(remote, &list_script(remote))?;
        let mut output = Vec::new();
        if let Some(stdout) = running.stdout.take() {
            stdout
                .take(MAX_LISTING_BYTES + 1)
                .read_to_end(&mut output)?;
        }
        if output.len() as u64 > MAX_LISTING_BYTES {
            return Err(remote_error(
                &remote.host,
                "the list of files is too long. Use narrower paths",
            ));
        }
        running.finish()?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    fn download(
        &self,
        remote: &Remote,
        path: &str,
        size: u64,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
        cancel: &AtomicBool,
    ) -> Result<u64> {
        let mut running = self.run(remote, &download_script(path, size)?)?;
        let mut total = 0u64;
        if let Some(mut stdout) = running.stdout.take() {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let n = stdout.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                // More than was asked for is not the file: stop before it fills the disk.
                if total > size {
                    return Err(remote_error(
                        &remote.host,
                        format!(
                            "the server sent more than the {size} bytes of {path}. A login script on the server may be printing text; it must print nothing for non-interactive logins"
                        ),
                    ));
                }
                dest.write_all(&buf[..n])?;
                progress(total);
            }
        }
        running.finish()?;
        Ok(total)
    }
}

/// One line of the listing that named a usable file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    path: String,
    size: u64,
    mtime: i64,
}

/// Reads the listing output. Returns the files and notes about what was
/// left out.
fn parse_listing(output: &str, remote: &Remote) -> Result<(Vec<Listed>, Vec<String>)> {
    let mut lines = output.lines().map(|l| l.trim_end_matches('\r'));
    // Anything before the marker is a login script talking.
    if !lines.by_ref().any(|l| l == LIST_BEGIN) {
        return Err(remote_error(
            &remote.host,
            "the server did not return a file list. Check that the account can run `sh` and `stat`",
        ));
    }

    let mut files: Vec<Listed> = Vec::new();
    let mut known: HashSet<String> = HashSet::new();
    let mut matched = vec![0usize; remote.paths.len()];
    let mut current: Option<usize> = None;
    let (mut unreadable, mut unusable, mut garbled) = (0usize, 0usize, 0usize);
    let mut unreadable_example = String::new();
    let mut complete = false;

    for line in lines {
        if line == LIST_END {
            complete = true;
            break;
        }
        let count = |matched: &mut Vec<usize>| {
            if let Some(slot) = current.and_then(|i| matched.get_mut(i)) {
                *slot += 1;
            }
        };
        if let Some(index) = line.strip_prefix("P ") {
            current = index.parse().ok();
        } else if let Some(name) = line.strip_prefix("U ") {
            count(&mut matched);
            unreadable += 1;
            if unreadable_example.is_empty() {
                unreadable_example = printable(name, 200);
            }
        } else if let Some(rest) = line.strip_prefix("F ") {
            let mut parts = rest.splitn(3, ' ');
            let parsed = (|| {
                let size = parts.next()?.parse::<u64>().ok()?;
                let mtime = parts.next()?.parse::<i64>().ok()?;
                Some((size, mtime, parts.next()?))
            })();
            match parsed {
                None => garbled += 1,
                Some((size, mtime, name)) => {
                    count(&mut matched);
                    if !valid_name(name) {
                        unusable += 1;
                    } else if known.insert(name.to_owned()) {
                        if files.len() >= MAX_FILES {
                            return Err(remote_error(
                                &remote.host,
                                format!("more than {MAX_FILES} files match. Use narrower paths"),
                            ));
                        }
                        files.push(Listed {
                            path: name.to_owned(),
                            size,
                            mtime,
                        });
                    }
                }
            }
        } else if !line.trim().is_empty() {
            garbled += 1;
        }
    }
    if !complete {
        return Err(remote_error(
            &remote.host,
            "the file list was cut short. Try again",
        ));
    }

    let mut notes = Vec::new();
    for (pattern, _) in remote.paths.iter().zip(&matched).filter(|(_, n)| **n == 0) {
        notes.push(format!("Nothing on the server matches {pattern}"));
    }
    if unreadable > 0 {
        notes.push(format!(
            "{unreadable} file(s) cannot be read by this account, for example {unreadable_example}. \
             Log files are often readable only by a group such as adm; the server's administrator can add the account to it"
        ));
    }
    if unusable > 0 {
        notes.push(format!(
            "{unusable} file(s) were left out because their names contain spaces or other unusual characters"
        ));
    }
    if garbled > 0 {
        notes.push(format!(
            "{garbled} line(s) of the server's file list were not understood"
        ));
    }
    Ok((files, notes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileState {
    /// Never fetched.
    New,
    /// Fetched before, but its size or modification time is different now.
    Changed,
    /// Fetched and unchanged: skipped.
    Fetched,
    /// Not modified recently enough for the `days` option: skipped.
    Old,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteFile {
    pub path: String,
    pub size_bytes: u64,
    /// Unix seconds, UTC.
    pub modified: i64,
    pub state: FileState,
}

/// What is on the server, and what a fetch would do with each file.
#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub host: String,
    /// Oldest first, the order a fetch works in.
    pub files: Vec<RemoteFile>,
    pub notes: Vec<String>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn classify(
    listed: Vec<Listed>,
    seen: &SeenMap,
    ids: &HashSet<i32>,
    days: Option<u32>,
    force: bool,
    now: i64,
) -> Vec<RemoteFile> {
    let mut files: Vec<RemoteFile> = listed
        .into_iter()
        .map(|file| {
            // A record counts only while the file it became is still in the site.
            let state = match seen.get(&file.path) {
                Some(s) if s.file_id.is_none_or(|id| ids.contains(&id)) => {
                    if s.size == file.size && s.mtime == file.mtime && !force {
                        FileState::Fetched
                    } else {
                        FileState::Changed
                    }
                }
                _ => FileState::New,
            };
            let too_old = days.is_some_and(|d| now - file.mtime > i64::from(d) * 86_400);
            RemoteFile {
                path: file.path,
                size_bytes: file.size,
                modified: file.mtime,
                state: if too_old && state != FileState::Fetched {
                    FileState::Old
                } else {
                    state
                },
            }
        })
        .collect();
    files.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
    files
}

/// Lists the files on the site's server without fetching anything.
pub fn list(store: &Store, days: Option<u32>, transport: &dyn Transport) -> Result<Listing> {
    let remote = require(store)?;
    let (listed, notes) = parse_listing(&transport.list(&remote)?, &remote)?;
    let files = classify(
        listed,
        &load_seen(store)?,
        &file_ids(store)?,
        days,
        false,
        now_secs(),
    );
    Ok(Listing {
        host: remote.host,
        files,
        notes,
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FetchOptions {
    /// Leave out files not modified in the last this many days.
    pub days: Option<u32>,
    /// `force` here also fetches files again that have not changed.
    pub ingest: IngestOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Download,
    Ingest,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    pub phase: Phase,
    /// The path on the server.
    pub path: &'a str,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Rows added so far; zero while downloading.
    pub rows: u64,
    /// Which file this is, counting from 1, and of how many.
    pub file: usize,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchReport {
    pub host: String,
    /// One entry per file fetched, in the order they were ingested.
    pub files: Vec<FileReport>,
    /// Files on the server that were skipped because nothing had changed.
    pub already_fetched: usize,
    /// Files skipped by the `days` option.
    pub too_old: usize,
    pub bytes_downloaded: u64,
    pub notes: Vec<String>,
}

/// A temporary file in the data directory, deleted on drop.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn create(dir: &Path) -> Result<(TempFile, std::fs::File)> {
        std::fs::create_dir_all(dir)?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!("fetch-{}-{nanos}.part", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| Error::File {
                path: path.clone(),
                source,
            })?;
        Ok((TempFile { path }, file))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Removes downloads a crashed run left behind.
fn remove_stale_temp(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let stale = name.starts_with("fetch-")
            && name.ends_with(".part")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMP);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Fetches the new and changed files from the site's server and ingests
/// them, oldest first. Stops at the first error; files before it stay
/// committed.
pub fn fetch(
    store: &mut Store,
    opts: &FetchOptions,
    transport: &dyn Transport,
    progress: &mut dyn FnMut(Progress<'_>),
    cancel: &AtomicBool,
) -> Result<FetchReport> {
    need_write(store)?;
    let remote = require(store)?;
    let (listed, notes) = parse_listing(&transport.list(&remote)?, &remote)?;
    let mut seen = load_seen(store)?;
    let files = classify(
        listed,
        &seen,
        &file_ids(store)?,
        opts.days,
        opts.ingest.force,
        now_secs(),
    );
    let count = |state: FileState| files.iter().filter(|f| f.state == state).count();
    let mut report = FetchReport {
        host: remote.host.clone(),
        files: Vec::new(),
        already_fetched: count(FileState::Fetched),
        too_old: count(FileState::Old),
        bytes_downloaded: 0,
        notes,
    };
    let wanted: Vec<&RemoteFile> = files
        .iter()
        .filter(|f| matches!(f.state, FileState::New | FileState::Changed))
        .collect();
    if wanted.is_empty() {
        return Ok(report);
    }

    // Forget files that are no longer on the server.
    seen.retain(|path, _| files.iter().any(|f| &f.path == path));

    let temp_dir = store.data_root().join("tmp");
    remove_stale_temp(&temp_dir);
    let source = remote.source();
    let clears = store.settings()?.retention == Retention::None;
    let mut session = ingest::Session::begin(store, &opts.ingest)?;
    if clears {
        // Retention `none` has just emptied the site. The files fetched
        // before must not look new next time, or two sets of files would
        // replace each other on alternate fetches.
        for entry in seen.values_mut() {
            entry.file_id = None;
        }
    }

    for (index, file) in wanted.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let position = |phase, bytes_done, bytes_total, rows| Progress {
            phase,
            path: &file.path,
            bytes_done,
            bytes_total,
            rows,
            file: index + 1,
            files: wanted.len(),
        };

        let (temp, mut dest) = TempFile::create(&temp_dir)?;
        progress(position(Phase::Download, 0, file.size_bytes, 0));
        let mut reported = 0u64;
        let got = transport.download(
            &remote,
            &file.path,
            file.size_bytes,
            &mut dest,
            &mut |done| {
                if done - reported >= PROGRESS_EVERY_BYTES || done == file.size_bytes {
                    reported = done;
                    progress(position(Phase::Download, done, file.size_bytes, 0));
                }
            },
            cancel,
        )?;
        dest.flush()?;
        drop(dest);
        if got != file.size_bytes {
            return Err(remote_error(
                &remote.host,
                format!(
                    "{} came back as {got} bytes instead of {}. It may have been rotated while it was being fetched; run the fetch again",
                    file.path, file.size_bytes
                ),
            ));
        }
        report.bytes_downloaded += got;

        let ingested = session.file(
            store,
            &temp.path,
            Some(Origin {
                source: &source,
                path: &file.path,
            }),
            &opts.ingest,
            &mut |p| progress(position(Phase::Ingest, p.bytes_done, p.bytes_total, p.rows)),
            cancel,
        )?;
        drop(temp);

        seen.insert(
            file.path.clone(),
            Seen {
                size: file.size_bytes,
                mtime: file.modified,
                file_id: ingested.file_id,
            },
        );
        save_seen(store, &seen)?;
        report.files.push(ingested);
    }

    session.finish(store, &opts.ingest)?;
    Ok(report)
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
