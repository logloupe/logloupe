//! Log sources. Every source is exposed as a streaming `BufRead`; nothing is
//! read whole or memory-mapped.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::bufread::MultiGzDecoder;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
const READ_BUFFER: usize = 1 << 20;

/// An opened log file, decompressed on the fly when it is gzip.
pub struct Source {
    pub reader: Box<dyn BufRead + Send>,
    /// Size on disk (the compressed size for gzip).
    pub size_bytes: u64,
    /// Bytes read from disk so far. Progress is `consumed / size_bytes`.
    pub consumed: Arc<AtomicU64>,
    pub gzip: bool,
}

/// Opens a local file. Gzip is detected by magic bytes, not by extension, and
/// multi-member gzip files are read to the end.
pub fn open(path: &Path) -> io::Result<Source> {
    let file = File::open(path)?;
    let size_bytes = file.metadata()?.len();
    let consumed = Arc::new(AtomicU64::new(0));
    let counting = CountingReader {
        inner: file,
        count: Arc::clone(&consumed),
    };
    let mut raw = BufReader::with_capacity(READ_BUFFER, counting);
    let gzip = raw.fill_buf()?.starts_with(&GZIP_MAGIC);
    let reader: Box<dyn BufRead + Send> = if gzip {
        Box::new(BufReader::with_capacity(
            READ_BUFFER,
            MultiGzDecoder::new(raw),
        ))
    } else {
        Box::new(raw)
    };
    Ok(Source {
        reader,
        size_bytes,
        consumed,
        gzip,
    })
}

struct CountingReader<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("loga-source-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    fn read_all(path: &Path) -> (String, Source) {
        let mut source = open(path).unwrap();
        let mut out = String::new();
        source.reader.read_to_string(&mut out).unwrap();
        (out, source)
    }

    #[test]
    fn plain_file() {
        let path = temp_file("plain.log", b"one\ntwo\n");
        let (text, source) = read_all(&path);
        assert_eq!(text, "one\ntwo\n");
        assert!(!source.gzip);
        assert_eq!(source.consumed.load(Ordering::Relaxed), source.size_bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn gzip_detected_by_magic_not_extension() {
        let path = temp_file("looks-plain.log", &gz(b"one\ntwo\n"));
        let (text, source) = read_all(&path);
        assert_eq!(text, "one\ntwo\n");
        assert!(source.gzip);
        assert_eq!(source.consumed.load(Ordering::Relaxed), source.size_bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn multi_member_gzip() {
        let mut bytes = gz(b"one\n");
        bytes.extend(gz(b"two\n"));
        let path = temp_file("multi.gz", &bytes);
        let (text, _) = read_all(&path);
        assert_eq!(text, "one\ntwo\n");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_file() {
        let path = temp_file("empty.log", b"");
        let (text, source) = read_all(&path);
        assert_eq!(text, "");
        assert!(!source.gzip);
        std::fs::remove_file(path).unwrap();
    }
}
