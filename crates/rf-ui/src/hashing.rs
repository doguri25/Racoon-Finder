//! The checksums of the properties window (OPS-10): MD5, SHA-1, SHA-256, BLAKE3 and
//! xxHash3-64 of its files, made on a worker thread ([`hash_files`]) and compared with a hash
//! the user pastes ([`same_digest`]).
//!
//! The UI thread never opens a file: it asks for a [`HashRequest`] with a generation
//! ([`Effect::Hash`](crate::state::Effect::Hash)), the files in the order of the window's rows,
//! and the worker reads them one after the other, [`HASH_CHUNK`] at a time into one buffer,
//! telling how far it is ([`HashNews::Reading`]) and what the files gave ([`HashNews::Hashed`],
//! those of many small files together), every [`PROGRESS_GAP`] at most. It looks at its
//! [`CancelToken`] before every chunk: a newer request (another algorithm, another window), the
//! window's 중지 or its end stops it within a chunk, and the state takes only the news with the
//! generation it waits for.
//!
//! A file is opened only for reading, through the provider, whose `open_read` refuses at once
//! anything that is no regular file (a FIFO, whose reader would wait for its writer, a device, a
//! folder): such an item gives [`HashFailure::NotAFile`].

use std::io::{self, Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

use md5::Md5;
use rf_core::{Error, ErrorKind, VPath};
use rf_vfs::{CancelToken, Vfs};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use xxhash_rust::xxh3::Xxh3;

/// How much of a file is read at a time: 1 MB.
pub const HASH_CHUNK: usize = 1024 * 1024;

/// The shortest time between two reports of how far a worker is (of the hashes and of the
/// folder count), so that a fast disk does not flood the UI thread.
pub const PROGRESS_GAP: Duration = Duration::from_millis(100);

/// What a checksum is made with. The numbers ([`Algorithm::number`]) are the places of the
/// window's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Algorithm {
    Md5,
    Sha1,
    Sha256,
    Blake3,
    /// xxHash3 with 64 bits and the seed 0: the 16 hexadecimal digits `xxhsum -H3` writes
    /// after its `XXH3_` ([`same_digest`] takes them with it or without).
    Xxh3,
}

impl Algorithm {
    /// Every algorithm, in the order of the window's choices.
    pub const ALL: [Algorithm; 5] = [
        Algorithm::Md5,
        Algorithm::Sha1,
        Algorithm::Sha256,
        Algorithm::Blake3,
        Algorithm::Xxh3,
    ];

    /// The place of the algorithm among the window's choices.
    pub fn number(self) -> i32 {
        match self {
            Algorithm::Md5 => 0,
            Algorithm::Sha1 => 1,
            Algorithm::Sha256 => 2,
            Algorithm::Blake3 => 3,
            Algorithm::Xxh3 => 4,
        }
    }

    /// The algorithm at place `number`; `None` for any other number.
    pub fn from_number(number: i32) -> Option<Algorithm> {
        Algorithm::ALL
            .into_iter()
            .find(|algorithm| algorithm.number() == number)
    }
}

/// A checksum being made.
enum Digester {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    // The two large states live on the heap, which keeps the others small.
    Blake3(Box<blake3::Hasher>),
    Xxh3(Box<Xxh3>),
}

impl Digester {
    fn new(algorithm: Algorithm) -> Digester {
        match algorithm {
            Algorithm::Md5 => Digester::Md5(Md5::new()),
            Algorithm::Sha1 => Digester::Sha1(Sha1::new()),
            Algorithm::Sha256 => Digester::Sha256(Sha256::new()),
            Algorithm::Blake3 => Digester::Blake3(Box::new(blake3::Hasher::new())),
            Algorithm::Xxh3 => Digester::Xxh3(Box::new(Xxh3::new())),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Digester::Md5(md5) => md5.update(bytes),
            Digester::Sha1(sha1) => sha1.update(bytes),
            Digester::Sha256(sha256) => sha256.update(bytes),
            Digester::Blake3(blake3) => {
                blake3.update(bytes);
            }
            Digester::Xxh3(xxh3) => xxh3.update(bytes),
        }
    }

    /// The checksum in lowercase hexadecimal digits, as `sha256sum`, `b3sum` and `xxhsum`
    /// write it (xxHash3's 64 bits big-endian, its canonical form).
    fn finish(self) -> String {
        match self {
            Digester::Md5(md5) => hex(&md5.finalize()),
            Digester::Sha1(sha1) => hex(&sha1.finalize()),
            Digester::Sha256(sha256) => hex(&sha256.finalize()),
            Digester::Blake3(blake3) => hex(blake3.finalize().as_bytes()),
            Digester::Xxh3(xxh3) => format!("{:016x}", xxh3.digest()),
        }
    }
}

/// `bytes` in lowercase hexadecimal digits, two a byte.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Why [`digest_stream`] gave no checksum.
#[derive(Debug)]
pub enum Stopped {
    /// Its token was cancelled.
    Cancelled,
    /// The stream could not be read.
    Failed(io::Error),
}

/// The checksum of what `reader` holds, made with `algorithm`, reading into `buffer` (as much as
/// it holds at a time; an empty one reads nothing and fails). `read` hears after every chunk how
/// many bytes have been read so far. It looks at `cancel` before each chunk, and stops there once
/// it is cancelled.
pub fn digest_stream(
    reader: &mut dyn Read,
    algorithm: Algorithm,
    buffer: &mut [u8],
    cancel: &CancelToken,
    read: &mut dyn FnMut(u64),
) -> Result<String, Stopped> {
    if buffer.is_empty() {
        return Err(Stopped::Failed(io::ErrorKind::InvalidInput.into()));
    }
    let mut digester = Digester::new(algorithm);
    let mut done: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(Stopped::Cancelled);
        }
        let read_now = match reader.read(buffer) {
            Ok(0) => break,
            Ok(read_now) => read_now,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Stopped::Failed(error)),
        };
        digester.update(&buffer[..read_now]);
        done = done.saturating_add(u64::try_from(read_now).unwrap_or(u64::MAX));
        read(done);
    }
    Ok(digester.finish())
}

/// What the state asks the worker to hash: the files of the window's rows, each with the
/// number of its row, with `algorithm`, under `generation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashRequest {
    pub generation: u64,
    pub algorithm: Algorithm,
    pub files: Vec<(usize, VPath)>,
}

/// What the worker tells of a [`HashRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HashNews {
    /// The file of row `row` is being read: `done` of its `total` bytes.
    Reading {
        generation: u64,
        row: usize,
        done: u64,
        total: u64,
    },
    /// The files of these rows gave these checksums, or none for these reasons: the rows that
    /// finished since the news before, in order.
    Hashed {
        generation: u64,
        results: Vec<(usize, Result<String, HashFailure>)>,
    },
}

/// Why a file has no checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashFailure {
    /// It is no regular file (any more): a folder, a FIFO, a device. It was not read.
    NotAFile,
    /// It could not be opened or read.
    Unreadable(ErrorKind),
}

/// Hashes the files of `request` through `vfs`, one after the other, `chunk` bytes at a time
/// (into one buffer for all of them), and tells `tell` how far it is and what the files gave: at
/// most every `gap`, with the checksums made since the news before together, and the last ones
/// at the end. Blocks: call it on a worker thread. `None` once `cancel` stopped it, which it
/// looks at before each file and each chunk; what it had not told by then is dropped.
pub fn hash_files(
    request: &HashRequest,
    vfs: &Vfs,
    cancel: &CancelToken,
    chunk: usize,
    gap: Duration,
    tell: &mut dyn FnMut(HashNews),
) -> Option<()> {
    let mut buffer = vec![0; chunk.max(1)];
    let mut news = Teller {
        generation: request.generation,
        tell,
        told: Instant::now(),
        gap,
        hashed: Vec::new(),
    };
    for (row, path) in &request.files {
        let row = *row;
        if cancel.is_cancelled() {
            return None;
        }
        let mut stream = match vfs
            .provider(path)
            .and_then(|provider| provider.open_read(path))
        {
            Ok(stream) => stream,
            Err(error) => {
                let refused = matches!(
                    &error,
                    Error::Io { source, .. } if rf_platform::is_not_a_regular_file(source)
                );
                news.hashed(
                    row,
                    Err(if refused {
                        HashFailure::NotAFile
                    } else {
                        HashFailure::Unreadable(error.kind())
                    }),
                );
                continue;
            }
        };
        let length = stream.seek(SeekFrom::End(0)).and_then(|length| {
            stream.rewind()?;
            Ok(length)
        });
        let total = match length {
            Ok(total) => total,
            Err(error) => {
                let kind = Error::io("seek", path.clone(), error).kind();
                news.hashed(row, Err(HashFailure::Unreadable(kind)));
                continue;
            }
        };
        // A file opened while a report is due says so at once; most small files do not.
        news.reading(row, 0, total);
        let digest = digest_stream(
            &mut stream,
            request.algorithm,
            &mut buffer,
            cancel,
            &mut |done| news.reading(row, done, total),
        );
        match digest {
            Ok(digest) => news.hashed(row, Ok(digest)),
            Err(Stopped::Cancelled) => return None,
            Err(Stopped::Failed(error)) => {
                let kind = Error::io("read", path.clone(), error).kind();
                news.hashed(row, Err(HashFailure::Unreadable(kind)));
            }
        }
    }
    news.flush();
    Some(())
}

/// The news of [`hash_files`] on their way to `tell`: at most every `gap`, the checksums made
/// since the news before together.
struct Teller<'a> {
    generation: u64,
    tell: &'a mut dyn FnMut(HashNews),
    /// When the last news went.
    told: Instant,
    gap: Duration,
    /// The rows finished and not told yet, in order.
    hashed: Vec<(usize, Result<String, HashFailure>)>,
}

impl Teller<'_> {
    /// Whether news is due: `gap` has passed since the last; then the time starts again.
    fn due(&mut self) -> bool {
        if self.told.elapsed() >= self.gap {
            self.told = Instant::now();
            return true;
        }
        false
    }

    /// The file of row `row` has been read up to `done` of its `total` bytes: told when news is
    /// due, after the checksums made before it.
    fn reading(&mut self, row: usize, done: u64, total: u64) {
        if self.due() {
            self.send_hashed();
            (self.tell)(HashNews::Reading {
                generation: self.generation,
                row,
                done,
                total,
            });
        }
    }

    /// Row `row` has finished: told with the others when news is due.
    fn hashed(&mut self, row: usize, result: Result<String, HashFailure>) {
        self.hashed.push((row, result));
        if self.due() {
            self.send_hashed();
        }
    }

    /// Tells the rows finished and not told yet, if any.
    fn send_hashed(&mut self) {
        if !self.hashed.is_empty() {
            (self.tell)(HashNews::Hashed {
                generation: self.generation,
                results: std::mem::take(&mut self.hashed),
            });
        }
    }

    /// Tells what is left: the end of the request.
    fn flush(&mut self) {
        self.send_hashed();
    }
}

/// Whether `pasted`, a hash the user pasted, is `digest`, ignoring the case of its letters.
/// It takes the first line of what is pasted, with the white space around it, in the forms the
/// tools write: the hash alone; the hash first and the file's name after it (`sha256sum`,
/// `b3sum`, `md5sum`, `xxhsum`, with `\` before a hash whose name they escaped); the BSD form
/// with the hash after ` = ` (`SHA256 (a.txt) = …`, as `shasum --tag`, `xxhsum --tag` and
/// macOS's `md5` write it); and `xxhsum -H3`'s `XXH3_` before the hash. The hash must then be
/// the whole of `digest`: a part of it never matches. `None` while there is nothing to compare
/// (only white space).
pub fn same_digest(pasted: &str, digest: &str) -> Option<bool> {
    let line = pasted.trim().lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let hash = match line.rsplit_once(" = ") {
        // The BSD form: `TAG (name) = hash`.
        Some((tag, hash)) if tag.ends_with(')') => hash.trim(),
        // The hash, then two spaces (or a space and `*`) and the name.
        _ => line.split_whitespace().next().unwrap_or(line),
    };
    let hash = hash.strip_prefix('\\').unwrap_or(hash);
    let hash = match hash.get(..5) {
        Some(prefix) if prefix.eq_ignore_ascii_case("XXH3_") => &hash[5..],
        _ => hash,
    };
    Some(!hash.is_empty() && hash.eq_ignore_ascii_case(digest.trim()))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// The checksums of `data` with every algorithm, in the order of [`Algorithm::ALL`].
    fn digests(data: &[u8], chunk: usize) -> Vec<String> {
        Algorithm::ALL
            .into_iter()
            .map(|algorithm| {
                digest_stream(
                    &mut Cursor::new(data),
                    algorithm,
                    &mut vec![0; chunk],
                    &CancelToken::new(),
                    &mut |_| {},
                )
                .expect("a stream in memory")
            })
            .collect()
    }

    /// 1 MB and one byte, `i % 251` at place `i` (the pattern of BLAKE3's test vectors): the
    /// last byte is in a chunk of its own.
    fn past_a_chunk() -> Vec<u8> {
        (0..=HASH_CHUNK)
            .map(|i| u8::try_from(i % 251).expect("below 251"))
            .collect()
    }

    // The vectors come from other implementations: Python's `hashlib` (OpenSSL) for MD5,
    // SHA-1 and SHA-256, the `blake3` and `xxhash` packages (the C reference of xxHash) for the
    // others.
    #[test]
    fn every_algorithm_gives_its_known_vectors() {
        let cases: [(&[u8], [&str; 5]); 2] = [
            (
                b"",
                [
                    "d41d8cd98f00b204e9800998ecf8427e",
                    "da39a3ee5e6b4b0d3255bfef95601890afd80709",
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
                    "2d06800538d394c2",
                ],
            ),
            (
                b"abc",
                [
                    "900150983cd24fb0d6963f7d28e17f72",
                    "a9993e364706816aba3e25717850c26c9cd0d89d",
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                    "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
                    "78af5f94892f3950",
                ],
            ),
        ];
        for (data, wanted) in cases {
            assert_eq!(digests(data, HASH_CHUNK), wanted, "{data:?}");
        }
    }

    #[test]
    fn a_stream_past_a_chunk_gives_its_known_vectors_in_chunks_of_any_size() {
        let data = past_a_chunk();
        let wanted = [
            "79b67c7fbf43b76e5b7f182328bdc4b6",
            "ffbc30b06f6bd51da52f9640804c9d0c4f6e2af2",
            "5769f52bc3eef28afa39c6fc68cadb7d0bd69812ae3a3d71452f519ec3c7aa56",
            "2f053cd7472cf0cd2f9adaf45c1180255b91b9a865404a63671a0ee5f792ed33",
            "47a84c196fd973df",
        ];
        assert_eq!(digests(&data, HASH_CHUNK), wanted);
        // The chunks change nothing but how often it is told.
        assert_eq!(digests(&data, 4096 + 7), wanted);
        let mut told = Vec::new();
        digest_stream(
            &mut Cursor::new(&data),
            Algorithm::Sha256,
            &mut vec![0; HASH_CHUNK],
            &CancelToken::new(),
            &mut |done| told.push(done),
        )
        .expect("a stream in memory");
        assert_eq!(told, [1 << 20, (1 << 20) + 1]);
        // A buffer with no room reads nothing, and says so rather than give the empty stream's.
        let empty = digest_stream(
            &mut Cursor::new(&data),
            Algorithm::Sha256,
            &mut [],
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(matches!(empty, Err(Stopped::Failed(_))));
    }

    /// A reader that never ends, as a file of many gigabytes would not for a long time, and
    /// that counts how much it gave.
    struct Endless<'a>(&'a AtomicU64);

    impl Read for Endless<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            buffer.fill(7);
            self.0.fetch_add(buffer.len() as u64, Ordering::Relaxed);
            Ok(buffer.len())
        }
    }

    #[test]
    fn a_cancel_stops_a_large_stream_within_a_chunk() {
        for algorithm in Algorithm::ALL {
            let given = AtomicU64::new(0);
            let cancel = CancelToken::new();
            let mut chunks = 0;
            let result = digest_stream(
                &mut Endless(&given),
                algorithm,
                &mut vec![0; HASH_CHUNK],
                &cancel,
                &mut |_| {
                    chunks += 1;
                    if chunks == 3 {
                        // The window closed while the third chunk was hashed.
                        cancel.cancel();
                    }
                },
            );
            assert!(matches!(result, Err(Stopped::Cancelled)), "{algorithm:?}");
            // Not a byte past the chunk under way.
            assert_eq!(given.load(Ordering::Relaxed), 3 << 20, "{algorithm:?}");
        }
    }

    #[test]
    fn a_stream_that_fails_says_why() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        let result = digest_stream(
            &mut Broken,
            Algorithm::Md5,
            &mut vec![0; HASH_CHUNK],
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(
            matches!(result, Err(Stopped::Failed(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn a_pasted_hash_is_compared_without_case_and_the_space_around_it() {
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for pasted in [
            digest,
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
            "  ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\r\n",
            "\tBa7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015AD ",
        ] {
            assert_eq!(same_digest(pasted, digest), Some(true), "{pasted:?}");
        }
        for pasted in [
            // A digit off, one short, a space inside, another algorithm's.
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ae",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a",
            "ba7816bf8f01cfea414140de5dae2223 b00361a396177a9cb410ff61f20015ad",
            "900150983cd24fb0d6963f7d28e17f72",
        ] {
            assert_eq!(same_digest(pasted, digest), Some(false), "{pasted:?}");
        }
        assert_eq!(same_digest("", digest), None);
        assert_eq!(same_digest(" \n\t", digest), None);
    }

    #[test]
    fn a_pasted_line_of_a_checksum_tool_is_compared_by_its_hash() {
        let sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for pasted in [
            // `sha256sum`, `b3sum`: the hash, two spaces, the name; `*` for binary mode.
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  abc.txt\n",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad *abc.txt",
            // A name with a space or a ` = ` in it changes nothing.
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  a = b.txt",
            // A name the tool escaped: `\` before the hash.
            "\\ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a\\nb",
            // The BSD form (`shasum --tag`, `xxhsum --tag`, macOS's `md5`).
            "SHA256 (abc.txt) = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA256 (a = b.txt) = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            // The first of several lines.
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a\nffff  b\n",
        ] {
            assert_eq!(same_digest(pasted, sha256), Some(true), "{pasted:?}");
        }
        // `xxhsum -H3`: `XXH3_` before the hash, in either case, and its BSD form.
        let xxh3 = "78af5f94892f3950";
        for pasted in [
            "XXH3_78af5f94892f3950  abc.txt",
            "xxh3_78AF5F94892F3950",
            "XXH3 (abc.txt) = 78af5f94892f3950",
        ] {
            assert_eq!(same_digest(pasted, xxh3), Some(true), "{pasted:?}");
        }
        for (pasted, digest) in [
            // The hash of another line, a part of the hash, no hash after the tool's words.
            (
                "ffff  a\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  b",
                sha256,
            ),
            ("ba7816bf8f01cfea  abc.txt", sha256),
            ("SHA256 (abc.txt) = ba7816bf", sha256),
            ("SHA256 (abc.txt) = ", sha256),
            ("XXH3_", xxh3),
            ("XXH3_78af5f94892f39", xxh3),
            ("XXH64_78af5f94892f3950", xxh3),
        ] {
            assert_eq!(same_digest(pasted, digest), Some(false), "{pasted:?}");
        }
    }

    /// A temporary folder with the file `name` holding `data`.
    fn temp_file(name: &str, data: &[u8]) -> (tempfile::TempDir, VPath) {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let path = dir.path().join(name);
        std::fs::write(&path, data).expect("the file is written");
        (dir, VPath::local(path).expect("a local path"))
    }

    #[test]
    fn the_files_of_a_request_are_hashed_in_turn_and_tell_how_far_they_are() {
        let (dir, abc) = temp_file("abc.txt", b"abc");
        let empty = VPath::local(dir.path().join("empty")).expect("a local path");
        std::fs::write(empty.as_local().expect("local"), b"").expect("the file is written");
        let gone = VPath::local(dir.path().join("gone")).expect("a local path");
        let request = HashRequest {
            generation: 7,
            algorithm: Algorithm::Sha1,
            files: vec![(2, abc), (4, gone), (9, empty)],
        };
        let mut news = Vec::new();
        let done = hash_files(
            &request,
            &Vfs::with_local(),
            &CancelToken::new(),
            2,
            Duration::ZERO,
            &mut |item| news.push(item),
        );
        assert_eq!(done, Some(()));
        let hashed: Vec<(usize, Result<String, HashFailure>)> = news
            .iter()
            .filter_map(|item| match item {
                HashNews::Hashed {
                    generation: 7,
                    results,
                } => Some(results.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            hashed,
            [
                (2, Ok("a9993e364706816aba3e25717850c26c9cd0d89d".to_owned())),
                (4, Err(HashFailure::Unreadable(ErrorKind::NotFound))),
                (9, Ok("da39a3ee5e6b4b0d3255bfef95601890afd80709".to_owned())),
            ]
        );
        // Without a gap, every chunk of the file of three bytes, two bytes a chunk, is told.
        let reading: Vec<(u64, u64)> = news
            .iter()
            .filter_map(|item| match item {
                HashNews::Reading {
                    generation: 7,
                    row: 2,
                    done,
                    total,
                } => Some((*done, *total)),
                _ => None,
            })
            .collect();
        assert_eq!(reading, [(0, 3), (2, 3), (3, 3)]);
    }

    #[test]
    fn a_cancel_stops_the_files_of_a_request_in_the_middle_of_a_file() {
        // A small file read in small chunks stands in for a large one.
        let (_dir, file) = temp_file("large.bin", &[1; 64]);
        let request = HashRequest {
            generation: 3,
            algorithm: Algorithm::Blake3,
            files: vec![(0, file.clone()), (1, file)],
        };
        let cancel = CancelToken::new();
        let mut news = Vec::new();
        let done = hash_files(
            &request,
            &Vfs::with_local(),
            &cancel,
            8,
            Duration::ZERO,
            &mut |item| {
                if matches!(item, HashNews::Reading { done: 16, .. }) {
                    cancel.cancel();
                }
                news.push(item);
            },
        );
        assert_eq!(done, None);
        // Nothing hashed, and not a chunk read after the one under way.
        assert!(
            !news
                .iter()
                .any(|item| matches!(item, HashNews::Hashed { .. })),
            "{news:?}"
        );
        assert!(matches!(
            news.last(),
            Some(HashNews::Reading {
                row: 0,
                done: 16,
                ..
            })
        ));
    }

    #[test]
    fn the_checksums_of_many_small_files_come_together() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let files: Vec<(usize, VPath)> = (0..50)
            .map(|i| {
                let path = dir.path().join(format!("f{i}"));
                std::fs::write(&path, [u8::try_from(i).expect("below 50")]).expect("a file");
                (i, VPath::local(path).expect("a local path"))
            })
            .collect();
        let request = HashRequest {
            generation: 4,
            algorithm: Algorithm::Xxh3,
            files,
        };
        let mut news = Vec::new();
        hash_files(
            &request,
            &Vfs::with_local(),
            &CancelToken::new(),
            HASH_CHUNK,
            Duration::from_secs(3600),
            &mut |item| news.push(item),
        );
        // Within the gap, one piece of news with all fifty, in order, and no progress.
        let [HashNews::Hashed { results, .. }] = news.as_slice() else {
            panic!("one piece of news, not {news:?}");
        };
        let rows: Vec<usize> = results.iter().map(|(row, _)| *row).collect();
        assert_eq!(rows, (0..50).collect::<Vec<_>>());
        assert!(results.iter().all(|(_, result)| result.is_ok()));
    }

    // Unix opens a folder or a FIFO and refuses it as no regular file; Windows does not open a
    // folder as a file at all, and has no FIFO in a folder.
    #[cfg(unix)]
    #[test]
    fn a_fifo_and_a_folder_are_refused_at_once() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let fifo = dir.path().join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs");
        assert!(made.success());
        let request = HashRequest {
            generation: 1,
            algorithm: Algorithm::Md5,
            files: vec![
                (0, VPath::local(&fifo).expect("a local path")),
                (1, VPath::local(dir.path()).expect("a local path")),
            ],
        };
        let mut news = Vec::new();
        // A reader of a FIFO would wait here for its writer forever.
        hash_files(
            &request,
            &Vfs::with_local(),
            &CancelToken::new(),
            HASH_CHUNK,
            Duration::from_secs(3600),
            &mut |item| news.push(item),
        );
        assert_eq!(
            news,
            [HashNews::Hashed {
                generation: 1,
                results: vec![
                    (0, Err(HashFailure::NotAFile)),
                    (1, Err(HashFailure::NotAFile))
                ],
            }]
        );
    }

    #[test]
    fn the_numbers_of_the_algorithms_are_their_places() {
        for (place, algorithm) in Algorithm::ALL.into_iter().enumerate() {
            let number = i32::try_from(place).expect("five places");
            assert_eq!(algorithm.number(), number);
            assert_eq!(Algorithm::from_number(number), Some(algorithm));
        }
        assert_eq!(Algorithm::from_number(-1), None);
        assert_eq!(Algorithm::from_number(5), None);
    }
}
