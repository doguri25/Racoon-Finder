//! The checksums of the properties window (OPS-10): MD5, SHA-1, SHA-256, BLAKE3 and
//! xxHash3-64 of its files, made on a worker thread ([`hash_files`]) and compared with a hash
//! the user pastes ([`same_digest`]).
//!
//! The UI thread never opens a file: it asks for a [`HashRequest`] with a generation
//! ([`Effect::Hash`](crate::state::Effect::Hash)), the files in the order of the window's rows,
//! and the worker reads them one after the other, [`HASH_CHUNK`] at a time, telling how far it
//! is ([`HashNews::Reading`], every [`PROGRESS_GAP`] at most) and what each one gave
//! ([`HashNews::Hashed`]). It looks at its [`CancelToken`] before every chunk: a newer request
//! (another algorithm, another window) or the window's end stops it within a chunk, and the
//! state takes only the news with the generation it waits for.
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
    /// xxHash3 with 64 bits and the seed 0, as `xxhsum -H3` writes it.
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

/// The checksum of what `reader` holds, made with `algorithm`, reading `chunk` bytes at a time
/// (at least one). `read` hears after every chunk how many bytes have been read so far. It looks
/// at `cancel` before each chunk, and stops there once it is cancelled.
pub fn digest_stream(
    reader: &mut dyn Read,
    algorithm: Algorithm,
    chunk: usize,
    cancel: &CancelToken,
    read: &mut dyn FnMut(u64),
) -> Result<String, Stopped> {
    let mut buffer = vec![0; chunk.max(1)];
    let mut digester = Digester::new(algorithm);
    let mut done: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(Stopped::Cancelled);
        }
        let read_now = match reader.read(&mut buffer) {
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
    /// The file of row `row` gave this checksum, or none for that reason.
    Hashed {
        generation: u64,
        row: usize,
        result: Result<String, HashFailure>,
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

/// Hashes the files of `request` through `vfs`, one after the other, `chunk` bytes at a time,
/// and tells `tell` how far it is (at most every `gap`, and when a file is opened while that much
/// has passed) and what each file gave. Blocks: call it on a worker thread. `None` once `cancel`
/// stopped it, which it looks at before each file and each chunk.
pub fn hash_files(
    request: &HashRequest,
    vfs: &Vfs,
    cancel: &CancelToken,
    chunk: usize,
    gap: Duration,
    tell: &mut dyn FnMut(HashNews),
) -> Option<()> {
    let generation = request.generation;
    let mut told = Instant::now();
    for (row, path) in &request.files {
        let row = *row;
        if cancel.is_cancelled() {
            return None;
        }
        let failed = |kind: ErrorKind| HashNews::Hashed {
            generation,
            row,
            result: Err(HashFailure::Unreadable(kind)),
        };
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
                tell(if refused {
                    HashNews::Hashed {
                        generation,
                        row,
                        result: Err(HashFailure::NotAFile),
                    }
                } else {
                    failed(error.kind())
                });
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
                tell(failed(Error::io("seek", path.clone(), error).kind()));
                continue;
            }
        };
        let reading = |done: u64| HashNews::Reading {
            generation,
            row,
            done,
            total,
        };
        // A file opened while a report is due says so at once; most small files do not.
        if told.elapsed() >= gap {
            told = Instant::now();
            tell(reading(0));
        }
        let digest = digest_stream(&mut stream, request.algorithm, chunk, cancel, &mut |done| {
            if told.elapsed() >= gap {
                told = Instant::now();
                tell(reading(done));
            }
        });
        match digest {
            Ok(digest) => tell(HashNews::Hashed {
                generation,
                row,
                result: Ok(digest),
            }),
            Err(Stopped::Cancelled) => return None,
            Err(Stopped::Failed(error)) => {
                tell(failed(Error::io("read", path.clone(), error).kind()));
            }
        }
    }
    Some(())
}

/// Whether `pasted`, a hash the user pasted, is `digest`, ignoring the case of its letters and
/// the white space around it. `None` while there is nothing to compare (only white space).
pub fn same_digest(pasted: &str, digest: &str) -> Option<bool> {
    let pasted = pasted.trim();
    if pasted.is_empty() {
        return None;
    }
    Some(pasted.eq_ignore_ascii_case(digest.trim()))
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
                    chunk,
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
            HASH_CHUNK,
            &CancelToken::new(),
            &mut |done| told.push(done),
        )
        .expect("a stream in memory");
        assert_eq!(told, [1 << 20, (1 << 20) + 1]);
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
                HASH_CHUNK,
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
            HASH_CHUNK,
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
                    row,
                    result,
                } => Some((*row, result.clone())),
                _ => None,
            })
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
            PROGRESS_GAP,
            &mut |item| news.push(item),
        );
        assert_eq!(
            news,
            [0, 1].map(|row| HashNews::Hashed {
                generation: 1,
                row,
                result: Err(HashFailure::NotAFile)
            })
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
