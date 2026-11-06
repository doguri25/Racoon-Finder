//! The thumbnails of the icon views (VIEW-09): a pool of worker threads that makes them for the
//! items on screen, a memory cache of the last [`MEMORY_THUMBS`] and a disk cache under the app's
//! cache folder, and what the controller asks of it.
//!
//! The window never waits for a thumbnail. The controller hands the pool the files on screen
//! ([`ThumbPool::want`], in their order there: [`schedule`]), with the generation of the tab's
//! listing; a newer call drops what the last one asked for and was not started yet. The
//! controller takes what was made on a timer ([`ThumbPool::take_ready`]); what a listing other
//! than the newest asked for is dropped there.
//!
//! For each file a worker looks, in turn:
//!
//! 1. in memory (the last [`MEMORY_THUMBS`] made or read; this is done at once by
//!    [`ThumbPool::want`], without a worker);
//! 2. on disk: `<cache>/thumbs/<size>/<key>.png` ([`cache_key`]: the path, the length, the
//!    modification time and the size asked for, so a changed file is a new key);
//! 3. to the system's provider ([`rf_platform::thumbs::os_thumbnail`] in the app; a fake one in
//!    the tests), whose picture is made to fit the size asked (Windows may give a bigger one);
//! 4. when the system makes none, or fails, to the app's own decoder for the picture formats it
//!    knows ([`preview::image_format`], [`preview::decode_thumbnail_source`]): Linux has no
//!    provider, so every picture there is the app's. It keeps to the preview's file-size cap
//!    ([`preview::MAX_IMAGE_BYTES`]) and to tighter pixel bounds ([`preview::THUMB_PIXELS`]), and
//!    at most [`DECODES`] pictures are decoded at once in the whole app, however many pools
//!    there are.
//!
//! A thumbnail made is written to the disk cache (a new file only: written to a temporary name
//! with `create_new`, then renamed). "No thumbnail" is remembered in memory only, for the
//! generation of the listing (the system's "none" can pass: Windows' `E_FAIL` for a file whose
//! handler is busy), so the next listing of the folder asks again. The disk cache keeps to
//! [`CACHE_CAP`]: the oldest files go first ([`prune_plan`]), checked when the pool starts and
//! after every [`PRUNE_EVERY`] files written. The cache is the app's own data, and the only
//! files the UI code writes itself besides those of the settings and the logs; only the workers
//! touch it.
//!
//! The provider is other people's code, so every call is guarded: a panic fails that file only
//! ([`preview::guarded`]), and a watchdog gives up on a call that has taken longer than
//! [`THUMB_TIMEOUT`]: that file fails (and is not asked for again in the same listing, nor in
//! the next ones for [`TIMED_OUT_FOR`]), and a new worker takes the stuck one's place. The stuck
//! thread cannot be stopped; it is left to end when the call returns, and what it made then is
//! dropped. With [`MAX_STUCK`] such threads alive no more are started, and what is asked fails
//! at once until some return. The watchdog's clock runs only while the provider or the decoder
//! works: a file waiting for one of the [`DECODES`] decode permits is not timed (its clock
//! starts when it holds the permit), and that wait is bounded on its own by [`PERMIT_WAIT`]:
//! past it the request fails without being marked as timed out, and a later ask tries it again. Each worker is a plain thread of its
//! own (never a pool's): Windows' provider needs a thread that can join a single-threaded COM
//! apartment.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use image::{ImageFormat, RgbaImage};
use rf_core::{Attributes, Entry, EntryKind};
use rf_platform::thumbs::{ThumbOutcome, fit_within};
use rf_vfs::CancelToken;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::preview;

/// How long a worker may wait for one file's thumbnail before the watchdog gives up on it.
pub const THUMB_TIMEOUT: Duration = Duration::from_secs(5);
/// The most the disk cache holds, in bytes; the oldest files go first past it.
pub const CACHE_CAP: u64 = 256 * 1024 * 1024;
/// How many thumbnails the pool keeps in memory: the most recently made or read.
pub const MEMORY_THUMBS: usize = 512;
/// The disk cache is checked against [`CACHE_CAP`] after this many files written.
pub const PRUNE_EVERY: u64 = 1000;
/// The fewest and the most workers of a pool ([`worker_count`]).
pub const DEFAULT_WORKERS: usize = 2;
pub const MAX_WORKERS: usize = 4;
/// The folder of the disk cache under the app's cache folder.
pub const CACHE_FOLDER: &str = "thumbs";
/// How many pictures the app decodes at once, over every pool.
pub const DECODES: usize = 2;
/// How many stuck threads (calls the watchdog gave up on that have not returned) a pool lives
/// with before it stops starting workers.
pub const MAX_STUCK: usize = 8;
/// How long a file may wait for a decode permit; past it that request fails (and is asked for
/// again later: it is not marked as timed out).
pub const PERMIT_WAIT: Duration = Duration::from_secs(20);
/// How long a file that timed out is not asked for again, in any listing.
pub const TIMED_OUT_FOR: Duration = Duration::from_secs(60);

/// How often the watchdog looks at the workers.
const WATCH_STEP: Duration = Duration::from_millis(100);

/// What makes a file's thumbnail at a size in pixels: the system's provider in the app.
pub type ThumbProvider = Arc<dyn Fn(&Path, u32) -> ThumbOutcome + Send + Sync>;

/// A file on screen whose thumbnail is wanted: `size` pixels a side at most (the step's logical
/// size times the window's scale), and its place among the files on screen (0 first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbWant {
    pub path: PathBuf,
    pub size: u32,
    pub mtime: Option<SystemTime>,
    pub len: u64,
    pub visible_rank: usize,
}

/// What became of a [`ThumbWant`]: its picture, or `None` when the file has none (the system
/// and the app make none, the provider failed, panicked or took too long). The `generation` is
/// that of the [`ThumbPool::want`] that asked.
#[derive(Clone)]
pub struct ThumbReady {
    pub generation: u64,
    pub path: PathBuf,
    pub size: u32,
    /// The length and the modification time the file had when it was asked for.
    pub len: u64,
    pub mtime: Option<SystemTime>,
    pub image: Option<slint::Image>,
}

impl std::fmt::Debug for ThumbReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbReady")
            .field("generation", &self.generation)
            .field("path", &self.path)
            .field("size", &self.size)
            .field("len", &self.len)
            .field("mtime", &self.mtime)
            .field("image", &self.image.as_ref().map(slint::Image::size))
            .finish()
    }
}

/// The pixels of a thumbnail, which go between threads ([`slint::Image`] does not).
type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// A [`ThumbReady`] before the UI thread makes its image.
struct Done {
    generation: u64,
    path: PathBuf,
    size: u32,
    len: u64,
    mtime: Option<SystemTime>,
    pixels: Option<Pixels>,
}

/// A file to make, with its key and the generation that asked.
#[derive(Clone)]
struct Task {
    want: ThumbWant,
    key: String,
    generation: u64,
}

/// The pool's shared state.
struct Inner {
    state: Mutex<State>,
    /// Wakes the workers (a task, the end) and the watchdog (the end).
    wake: Condvar,
    cache: PathBuf,
    provider: ThumbProvider,
    workers: usize,
    timeout: Duration,
    max_stuck: usize,
    /// The decode permits to wait for, and how long.
    permits: &'static Permits,
    permit_wait: Duration,
}

struct State {
    /// The generation of the last [`ThumbPool::want`].
    generation: u64,
    /// The tasks not started, the first on screen first.
    queue: VecDeque<Task>,
    /// The tasks under way, by the id of their worker. A worker the watchdog gave up on is no
    /// longer here: it ends when its call returns. The time is when the call started, `None`
    /// while it waits for a decode permit (the watchdog does not time that).
    running: HashMap<u64, (Task, Option<Instant>)>,
    ready: Vec<Done>,
    /// The last thumbnails made or read, by key, with the time of their last use (a count).
    memory: HashMap<String, (Pixels, u64)>,
    uses: u64,
    /// The keys of the generation's files that have no thumbnail.
    none: HashSet<String>,
    /// The keys of the files that timed out, with when: not asked for again for
    /// [`TIMED_OUT_FOR`], whatever the listing.
    timed_out: HashMap<String, Instant>,
    /// The calls the watchdog gave up on that have not returned.
    stuck: usize,
    /// The workers that run and have not been given up on.
    alive: usize,
    next_id: u64,
    /// The watchdog runs (it starts with the first workers).
    watching: bool,
    closed: bool,
}

/// The files written to the disk cache by every pool, for [`PRUNE_EVERY`].
static WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Makes the thumbnails of the files on screen on worker threads; see the module.
pub struct ThumbPool {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ThumbPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbPool")
            .field("cache", &self.inner.cache)
            .field("workers", &self.inner.workers)
            .finish_non_exhaustive()
    }
}

impl ThumbPool {
    /// A pool of `workers` threads (at least one) that asks `provider` and keeps the thumbnails
    /// under `cache_dir`/[`CACHE_FOLDER`]. Its threads start with the first [`ThumbPool::want`]
    /// and end when it goes (a stuck one when its call returns).
    pub fn new(workers: usize, cache_dir: PathBuf, provider: ThumbProvider) -> ThumbPool {
        ThumbPool::with_limits(workers, cache_dir, provider, THUMB_TIMEOUT, MAX_STUCK)
    }

    /// [`ThumbPool::new`] with the watchdog's `timeout` and the most stuck threads, `max_stuck`.
    fn with_limits(
        workers: usize,
        cache_dir: PathBuf,
        provider: ThumbProvider,
        timeout: Duration,
        max_stuck: usize,
    ) -> ThumbPool {
        ThumbPool {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    generation: 0,
                    queue: VecDeque::new(),
                    running: HashMap::new(),
                    ready: Vec::new(),
                    memory: HashMap::new(),
                    uses: 0,
                    none: HashSet::new(),
                    timed_out: HashMap::new(),
                    stuck: 0,
                    alive: 0,
                    next_id: 0,
                    watching: false,
                    closed: false,
                }),
                wake: Condvar::new(),
                cache: {
                    let mut cache = cache_dir;
                    cache.push(CACHE_FOLDER);
                    cache
                },
                provider,
                workers: workers.max(1),
                timeout,
                max_stuck,
                permits: &DECODING,
                permit_wait: PERMIT_WAIT,
            }),
        }
    }

    /// The files on screen of the listing `folder_generation`, in place of what the last call
    /// asked for and was not started yet. A thumbnail in memory is ready at once; a file known
    /// to have none in this generation, or being made, is not asked for again. A new generation
    /// forgets which files had none.
    pub fn want(&self, folder_generation: u64, items: Vec<ThumbWant>) {
        let inner = &self.inner;
        let mut state = inner.lock();
        if state.generation != folder_generation {
            state.generation = folder_generation;
            state.none.clear();
        }
        state
            .timed_out
            .retain(|_, when| when.elapsed() < TIMED_OUT_FOR);
        state.queue.clear();
        for want in in_order(items) {
            let key = cache_key(&want.path, want.len, want.mtime, want.size);
            // A task under way for an older listing still fills the memory, but it does not
            // deliver: the file is asked again (and then found there).
            if state.none.contains(&key)
                || state.timed_out.contains_key(&key)
                || state
                    .running
                    .values()
                    .any(|(task, _)| task.key == key && task.generation == folder_generation)
            {
                continue;
            }
            if let Some(pixels) = state.remember(&key) {
                state.ready.push(Done {
                    generation: folder_generation,
                    path: want.path,
                    size: want.size,
                    len: want.len,
                    mtime: want.mtime,
                    pixels: Some(pixels),
                });
                continue;
            }
            state.queue.push_back(Task {
                want,
                key,
                generation: folder_generation,
            });
        }
        start_threads(inner, &mut state);
        shed(inner, &mut state);
        drop(state);
        inner.wake.notify_all();
    }

    /// What was made since the last call, for the UI thread (which makes the images).
    pub fn take_ready(&self) -> Vec<ThumbReady> {
        let done = std::mem::take(&mut self.inner.lock().ready);
        done.into_iter()
            .map(|done| ThumbReady {
                generation: done.generation,
                path: done.path,
                size: done.size,
                len: done.len,
                mtime: done.mtime,
                image: done.pixels.map(slint::Image::from_rgba8),
            })
            .collect()
    }

    /// Whether nothing waits, is made, or is ready to take.
    pub fn is_idle(&self) -> bool {
        let state = self.inner.lock();
        state.queue.is_empty() && state.running.is_empty() && state.ready.is_empty()
    }
}

impl Drop for ThumbPool {
    fn drop(&mut self) {
        self.inner.lock().closed = true;
        self.inner.wake.notify_all();
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // The lock is never held across the provider's call or a file's, so a poisoned one
        // holds whole data.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// The thumbnail of `key` in memory, now its last used.
    fn remember(&mut self, key: &str) -> Option<Pixels> {
        self.uses += 1;
        let uses = self.uses;
        let (pixels, used) = self.memory.get_mut(key)?;
        *used = uses;
        Some(pixels.clone())
    }

    /// Keeps `pixels` as `key`'s, forgetting the one used longest ago past [`MEMORY_THUMBS`].
    fn keep(&mut self, key: String, pixels: Pixels) {
        self.uses += 1;
        self.memory.insert(key, (pixels, self.uses));
        if self.memory.len() > MEMORY_THUMBS
            && let Some(oldest) = self
                .memory
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
        {
            self.memory.remove(&oldest);
        }
    }
}

/// Starts the workers the pool lacks, and the watchdog with the first ones (which prunes the
/// disk cache first). A thread the system cannot give is tried again at the next call.
fn start_threads(inner: &Arc<Inner>, state: &mut State) {
    if state.closed {
        return;
    }
    while state.alive < inner.workers && state.stuck < inner.max_stuck {
        state.next_id += 1;
        let id = state.next_id;
        let shared = Arc::clone(inner);
        let spawned = std::thread::Builder::new()
            .name("rf-ui-thumbs".to_owned())
            .spawn(move || work(&shared, id));
        if spawned.is_err() {
            break;
        }
        state.alive += 1;
    }
    if !state.watching {
        let shared = Arc::clone(inner);
        let spawned = std::thread::Builder::new()
            .name("rf-ui-thumbs-watch".to_owned())
            .spawn(move || watch(&shared));
        state.watching = spawned.is_ok();
    }
}

/// With no worker left and too many stuck threads for another, what waits fails at once (and
/// is not asked for again in the listing): a dead share must not leave a thread behind for
/// every file.
fn shed(inner: &Inner, state: &mut State) {
    if state.stuck < inner.max_stuck || state.alive > 0 {
        return;
    }
    let generation = state.generation;
    while let Some(task) = state.queue.pop_front() {
        state.none.insert(task.key);
        state.ready.push(Done {
            generation,
            path: task.want.path,
            size: task.want.size,
            len: task.want.len,
            mtime: task.want.mtime,
            pixels: None,
        });
    }
}

/// A worker: makes the first task of the queue until the pool goes, or the watchdog gave up
/// on it.
fn work(inner: &Arc<Inner>, id: u64) {
    loop {
        let task = {
            let mut state = inner.lock();
            loop {
                if state.closed {
                    state.alive = state.alive.saturating_sub(1);
                    return;
                }
                if let Some(task) = state.queue.pop_front() {
                    state
                        .running
                        .insert(id, (task.clone(), Some(Instant::now())));
                    break task;
                }
                state = inner
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        // What an older listing's task made is in memory: not made again.
        let known = inner.lock().remember(&task.key);
        let made = match known {
            Some(pixels) => Made::Pixels(pixels),
            None => make(inner, id, &task.want, &task.key),
        };
        let mut state = inner.lock();
        if state.running.remove(&id).is_none() {
            // Given up on: another worker has taken this one's place.
            state.stuck = state.stuck.saturating_sub(1);
            return;
        }
        let current = task.generation == state.generation;
        match &made {
            Made::Pixels(pixels) => state.keep(task.key, pixels.clone()),
            Made::Nothing if current => {
                state.none.insert(task.key);
            }
            // Not known to have none: a later ask tries it again.
            Made::Nothing | Made::Busy => {}
        }
        let pixels = match made {
            Made::Pixels(pixels) => Some(pixels),
            Made::Nothing | Made::Busy => None,
        };
        if current {
            state.ready.push(Done {
                generation: task.generation,
                path: task.want.path,
                size: task.want.size,
                len: task.want.len,
                mtime: task.want.mtime,
                pixels,
            });
        }
    }
}

/// The watchdog: starts the pruning of the disk cache, then gives up on each call that has taken longer
/// than the pool's timeout (its file fails, and is not asked for again in the generation) and
/// starts a worker in the stuck one's place (up to the most stuck threads), until the pool
/// goes.
fn watch(inner: &Arc<Inner>) {
    prune_in_background(&inner.cache);
    let step = WATCH_STEP
        .min(inner.timeout / 4)
        .max(Duration::from_millis(1));
    let mut state = inner.lock();
    loop {
        if state.closed {
            return;
        }
        state = inner
            .wake
            .wait_timeout(state, step)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        let stuck: Vec<u64> = state
            .running
            .iter()
            .filter(|(_, (_, started))| started.is_some_and(|at| at.elapsed() >= inner.timeout))
            .map(|(id, _)| *id)
            .collect();
        if stuck.is_empty() {
            continue;
        }
        for id in stuck {
            let Some((task, _)) = state.running.remove(&id) else {
                continue;
            };
            state.alive = state.alive.saturating_sub(1);
            state.stuck += 1;
            state.timed_out.insert(task.key.clone(), Instant::now());
            if task.generation == state.generation {
                state.none.insert(task.key);
                state.ready.push(Done {
                    generation: task.generation,
                    path: task.want.path,
                    size: task.want.size,
                    len: task.want.len,
                    mtime: task.want.mtime,
                    pixels: None,
                });
            }
        }
        start_threads(inner, &mut state);
        shed(inner, &mut state);
    }
}

/// What making a file's thumbnail came to.
enum Made {
    Pixels(Pixels),
    /// Nothing makes one.
    Nothing,
    /// No decode permit came in time: not known to have none.
    Busy,
}

impl Inner {
    /// Sets the clock of the call of worker `id`: `None` stops it (the worker waits for a
    /// permit), `Some` starts it now. Nothing when the watchdog gave up on the call.
    fn set_clock(&self, id: u64, running: bool) {
        if let Some((_, started)) = self.lock().running.get_mut(&id) {
            *started = running.then(Instant::now);
        }
    }
}

/// The thumbnail of `want`: from the disk cache, or the provider (guarded), or the app's own
/// decoder; written to the disk cache when it was made here. The watchdog's clock of worker `id`
/// stops while the decoder waits for a permit and starts again when it holds it.
fn make(inner: &Inner, id: u64, want: &ThumbWant, key: &str) -> Made {
    match make_pixels(inner, id, want, key) {
        Ok(Some(pixels)) => Made::Pixels(pixels),
        Ok(None) => Made::Nothing,
        Err(Busy) => Made::Busy,
    }
}

/// No decode permit came in time.
struct Busy;

fn make_pixels(
    inner: &Inner,
    id: u64,
    want: &ThumbWant,
    key: &str,
) -> Result<Option<Pixels>, Busy> {
    let file = cache_path(&inner.cache, want.size, key);
    if file.is_file() {
        match preview::guarded(|| read_cached(&file, want.size)).flatten() {
            Some(pixels) => return Ok(Some(pixels)),
            // A broken file is made again.
            None => {
                let _ = fs::remove_file(&file);
            }
        }
    }
    let provider = Arc::clone(&inner.provider);
    let Some(outcome) = preview::guarded(|| provider(&want.path, want.size)) else {
        return Ok(None);
    };
    let picture = match outcome {
        ThumbOutcome::Made(thumb) => RgbaImage::from_raw(thumb.width, thumb.height, thumb.rgba)
            .and_then(|image| fitted(&image, want.size)),
        ThumbOutcome::None | ThumbOutcome::Failed(_) => None,
    };
    let picture = if let Some(picture) = picture {
        picture
    } else {
        let busy = Cell::new(false);
        let acquire = || {
            inner.set_clock(id, false);
            let permit = inner.permits.acquire_within(inner.permit_wait);
            inner.set_clock(id, true);
            busy.set(permit.is_none());
            permit
        };
        let decoded = preview::guarded(|| decoded(&want.path, want.len, want.size, &acquire));
        if busy.get() {
            return Err(Busy);
        }
        match decoded.flatten() {
            Some(picture) => picture,
            None => return Ok(None),
        }
    };
    if write_cached(&file, &picture)
        && WRITTEN.fetch_add(1, Ordering::Relaxed) % PRUNE_EVERY == PRUNE_EVERY - 1
    {
        prune_in_background(&inner.cache);
    }
    Ok(Some(Pixels::clone_from_slice(
        picture.as_raw(),
        picture.width(),
        picture.height(),
    )))
}

/// `image` shrunk to fit `size` × `size` in its ratio (as it is when it fits); `None` for one
/// without area.
fn fitted(image: &RgbaImage, size: u32) -> Option<RgbaImage> {
    let (width, height) = fit_within(image.width(), image.height(), size);
    if width == 0 || height == 0 {
        return None;
    }
    if (width, height) == image.dimensions() {
        return Some(image.clone());
    }
    Some(image::imageops::thumbnail(image, width, height))
}

/// The app's own thumbnail of the picture at `path` (`len` bytes), by its extension; `None` for
/// a file that is not a picture it decodes, or is over the preview's file-size cap, or when
/// `acquire` gives no decode permit.
fn decoded(
    path: &Path,
    len: u64,
    size: u32,
    acquire: &dyn Fn() -> Option<Permit>,
) -> Option<RgbaImage> {
    let format = preview::image_format(path.file_name()?)?;
    if len > preview::MAX_IMAGE_BYTES {
        return None;
    }
    let reader = BufReader::new(File::open(path).ok()?);
    let _permit = acquire()?;
    let image = preview::decode_thumbnail_source(reader, format, &CancelToken::new()).ok()?;
    let rgba = RgbaImage::from_raw(
        image.pixels.width(),
        image.pixels.height(),
        image.pixels.as_bytes().to_vec(),
    )?;
    fitted(&rgba, size)
}

/// The thumbnail in the cache file `file`, when it is a PNG no bigger than `size`.
fn read_cached(file: &Path, size: u32) -> Option<Pixels> {
    let reader = BufReader::new(File::open(file).ok()?);
    let image = preview::decode_image(reader, ImageFormat::Png, &CancelToken::new()).ok()?;
    (image.pixels.width() <= size && image.pixels.height() <= size).then_some(image.pixels)
}

/// Writes `picture` to the cache file `file` as a PNG, when there is none: to a temporary name
/// beside it made with `create_new`, then renamed. Whether it wrote it.
fn write_cached(file: &Path, picture: &RgbaImage) -> bool {
    static TEMPS: AtomicU64 = AtomicU64::new(0);
    let Some(folder) = file.parent() else {
        return false;
    };
    if file.exists() || fs::create_dir_all(folder).is_err() {
        return false;
    }
    let mut bytes = Vec::new();
    if picture
        .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
        .is_err()
    {
        return false;
    }
    let temp = file.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        TEMPS.fetch_add(1, Ordering::Relaxed)
    ));
    let written = File::options()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut out| out.write_all(&bytes));
    if written.is_err() || fs::rename(&temp, file).is_err() {
        let _ = fs::remove_file(&temp);
        return false;
    }
    true
}

/// Whether a pruning runs now: one at a time.
static PRUNING: AtomicBool = AtomicBool::new(false);

/// [`prune`] of `cache` on a thread of its own, so that it takes no worker's or the watchdog's
/// time; nothing when one runs already.
fn prune_in_background(cache: &Path) {
    if PRUNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let cache = cache.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("rf-ui-thumbs-prune".to_owned())
        .spawn(move || {
            // Lets the next pruning start even if this one panics.
            let _done = PruningDone;
            prune(&cache, CACHE_CAP);
        });
    if spawned.is_err() {
        PRUNING.store(false, Ordering::Release);
    }
}

/// Ends the running pruning when dropped, on a panic too.
struct PruningDone;

impl Drop for PruningDone {
    fn drop(&mut self) {
        PRUNING.store(false, Ordering::Release);
    }
}

/// A counting semaphore: how many decodes of pictures run at once, over every pool.
struct Permits {
    free: Mutex<usize>,
    freed: Condvar,
}

/// A permit of [`DECODING`], given back when dropped.
struct Permit(&'static Permits);

static DECODING: Permits = Permits {
    free: Mutex::new(DECODES),
    freed: Condvar::new(),
};

impl Permits {
    /// Waits up to `wait` for a permit; `None` when none came.
    fn acquire_within(&'static self, wait: Duration) -> Option<Permit> {
        let deadline = Instant::now() + wait;
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        while *free == 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            free = self
                .freed
                .wait_timeout(free, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *free -= 1;
        Some(Permit(self))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.0.freed.notify_one();
    }
}

/// The cache file of `key` at `size` under the cache folder `cache`.
fn cache_path(cache: &Path, size: u32, key: &str) -> PathBuf {
    cache.join(size.to_string()).join(format!("{key}.png"))
}

/// The disk cache file of `want` under the app's cache folder `cache_dir`.
#[cfg(test)]
fn cache_file(cache_dir: &Path, want: &ThumbWant) -> PathBuf {
    let key = cache_key(&want.path, want.len, want.mtime, want.size);
    cache_path(&cache_dir.join(CACHE_FOLDER), want.size, &key)
}

/// Deletes the oldest files of the cache folder `cache` (the files of its size folders) until
/// they hold `cap` bytes at most ([`prune_plan`]). A file that cannot be read or deleted is
/// left.
fn prune(cache: &Path, cap: u64) {
    let Ok(sizes) = fs::read_dir(cache) else {
        return;
    };
    let mut entries = Vec::new();
    for size in sizes.flatten() {
        let Ok(files) = fs::read_dir(size.path()) else {
            continue;
        };
        for file in files.flatten() {
            if let Ok(meta) = file.metadata()
                && meta.is_file()
            {
                let time = meta.modified().unwrap_or(UNIX_EPOCH);
                entries.push((file.path(), meta.len(), time));
            }
        }
    }
    for path in prune_plan(&entries, cap) {
        let _ = fs::remove_file(path);
    }
}

/// How many workers a pool has: half the processors, [`DEFAULT_WORKERS`] at least and
/// [`MAX_WORKERS`] at most.
pub fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map_or(DEFAULT_WORKERS, |count| count.get() / 2)
        .clamp(DEFAULT_WORKERS, MAX_WORKERS)
}

/// The name of a thumbnail in the caches: the 128-bit XXH3 of the path, the length, the
/// modification time (to the nanosecond; none is its own) and the size, as 32 lowercase hex
/// digits. The same on every run of the same system.
pub fn cache_key(path: &Path, len: u64, mtime: Option<SystemTime>, size: u32) -> String {
    let mut bytes = Vec::with_capacity(path.as_os_str().len() + 40);
    bytes.extend_from_slice(path.as_os_str().as_encoded_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&len.to_le_bytes());
    match mtime {
        Some(time) => {
            let nanos = match time.duration_since(UNIX_EPOCH) {
                Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
                Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
            };
            bytes.push(1);
            bytes.extend_from_slice(&nanos.to_le_bytes());
        }
        None => bytes.push(0),
    }
    bytes.extend_from_slice(&size.to_le_bytes());
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes))
}

/// The files of the disk cache to delete so that the rest hold `cap` bytes at most: the oldest
/// first (by their time, then their path), no more than needed. `entries` are each file's path,
/// length and modification time.
pub fn prune_plan(entries: &[(PathBuf, u64, SystemTime)], cap: u64) -> Vec<PathBuf> {
    let mut total: u64 = entries.iter().map(|(_, len, _)| *len).sum();
    if total <= cap {
        return Vec::new();
    }
    let mut oldest: Vec<&(PathBuf, u64, SystemTime)> = entries.iter().collect();
    oldest.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    let mut plan = Vec::new();
    for (path, len, _) in oldest {
        if total <= cap {
            break;
        }
        total = total.saturating_sub(*len);
        plan.push(path.clone());
    }
    plan
}

/// The order in which `wants` are made: by their place on screen, the first first.
pub fn schedule(wants: &[ThumbWant]) -> Vec<ThumbWant> {
    in_order(wants.to_vec())
}

/// [`schedule`] of `wants` as they are handed over.
fn in_order(mut wants: Vec<ThumbWant>) -> Vec<ThumbWant> {
    wants.sort_by_key(|want| want.visible_rank);
    wants
}

/// What to ask of the pool for the rows `first..=last` (none when `last < first`) of `entries`, in the folder `folder`, at
/// `size` pixels: the files among them, ranked by their row. A folder keeps its icon, and a file
/// that is online only (`Attributes::OFFLINE`) is not downloaded for its picture.
pub fn visible_wants(
    entries: &[Entry],
    folder: &Path,
    first: usize,
    last: usize,
    size: u32,
) -> Vec<ThumbWant> {
    if last < first {
        return Vec::new();
    }
    entries
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first).saturating_add(1))
        .filter(|(_, entry)| {
            entry.kind == EntryKind::File && !entry.attributes.contains(Attributes::OFFLINE)
        })
        .map(|(row, entry)| ThumbWant {
            path: folder.join(&entry.name),
            size,
            mtime: entry.modified,
            len: entry.size,
            visible_rank: row - first,
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use rf_platform::thumbs::Thumb;

    use super::*;

    /// How long a test waits for what should come.
    const WAIT: Duration = Duration::from_secs(20);
    /// How long a test waits for what should not come.
    const QUIET: Duration = Duration::from_millis(300);

    fn want(dir: &Path, name: &str, rank: usize) -> ThumbWant {
        ThumbWant {
            path: dir.join(name),
            size: 48,
            mtime: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            len: 1234,
            visible_rank: rank,
        }
    }

    /// A thumbnail of `w` × `h` pixels of one color.
    fn made(w: u32, h: u32) -> ThumbOutcome {
        ThumbOutcome::Made(Thumb {
            width: w,
            height: h,
            rgba: [10, 20, 30, 255].repeat(usize::try_from(w * h).unwrap()),
        })
    }

    fn name_of(path: &Path) -> String {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
    }

    /// A file's name, the generation, and its picture's size.
    type Got = (String, u64, Option<(u32, u32)>);

    /// What the pool made, by file name, until `count` came or [`WAIT`] has passed: the
    /// generation, and the size of the picture (`None`: no thumbnail).
    fn collect(pool: &ThumbPool, count: usize) -> Vec<Got> {
        let deadline = Instant::now() + WAIT;
        let mut got = Vec::new();
        while got.len() < count && Instant::now() < deadline {
            for ready in pool.take_ready() {
                let size = ready.image.as_ref().map(|image| {
                    let size = image.size();
                    (size.width, size.height)
                });
                got.push((name_of(&ready.path), ready.generation, size));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        got
    }

    /// Nothing more comes for a while.
    fn nothing_more(pool: &ThumbPool) {
        std::thread::sleep(QUIET);
        let late: Vec<String> = pool
            .take_ready()
            .iter()
            .map(|ready| name_of(&ready.path))
            .collect();
        assert!(late.is_empty(), "{late:?}");
    }

    /// A provider that tells each file it is asked for and makes a 64 × 32 picture, except that
    /// it waits for `gate` before the file named `held`.
    fn telling(
        held: &'static str,
        gate: mpsc::Receiver<()>,
    ) -> (ThumbProvider, mpsc::Receiver<String>) {
        let (asked, asks) = mpsc::channel();
        let asked = Mutex::new(asked);
        let gate = Mutex::new(gate);
        let provider: ThumbProvider = Arc::new(move |path: &Path, _size: u32| {
            let name = name_of(path);
            let _ = asked.lock().unwrap().send(name.clone());
            if name == held {
                let _ = gate.lock().unwrap().recv_timeout(WAIT);
            }
            made(64, 32)
        });
        (provider, asks)
    }

    #[test]
    fn the_visible_items_are_made_in_the_order_they_show() {
        let dir = tempfile::tempdir().unwrap();
        let (_open, gate) = mpsc::channel();
        let (provider, asks) = telling("", gate);
        let pool = ThumbPool::new(1, dir.path().to_owned(), provider);
        let items = vec![
            want(dir.path(), "c.dat", 2),
            want(dir.path(), "a.dat", 0),
            want(dir.path(), "d.dat", 3),
            want(dir.path(), "b.dat", 1),
        ];
        pool.want(1, items);
        let got = collect(&pool, 4);
        let asked: Vec<String> = asks.try_iter().collect();
        assert_eq!(asked, ["a.dat", "b.dat", "c.dat", "d.dat"]);
        // A picture bigger than asked (Windows may give one) is made to fit: 64 × 32 in 48.
        assert!(
            got.iter()
                .all(|(_, generation, size)| *generation == 1 && *size == Some((48, 24))),
            "{got:?}"
        );
    }

    #[test]
    fn a_new_want_drops_the_requests_not_yet_started() {
        let dir = tempfile::tempdir().unwrap();
        let (open, gate) = mpsc::channel();
        let (provider, asks) = telling("a.dat", gate);
        let pool = ThumbPool::new(1, dir.path().to_owned(), provider);
        pool.want(
            1,
            vec![
                want(dir.path(), "a.dat", 0),
                want(dir.path(), "b.dat", 1),
                want(dir.path(), "c.dat", 2),
            ],
        );
        assert_eq!(asks.recv_timeout(WAIT).ok().as_deref(), Some("a.dat"));
        // Scrolled on: b and c are no longer on screen, d is.
        pool.want(1, vec![want(dir.path(), "d.dat", 0)]);
        open.send(()).unwrap();
        let mut got: Vec<String> = collect(&pool, 2)
            .into_iter()
            .map(|(name, ..)| name)
            .collect();
        got.sort();
        // The one under way is of the same folder: it still comes.
        assert_eq!(got, ["a.dat", "d.dat"]);
        nothing_more(&pool);
        let asked: Vec<String> = asks.try_iter().collect();
        assert_eq!(asked, ["d.dat"]);
    }

    #[test]
    fn what_another_generation_asked_for_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (open, gate) = mpsc::channel();
        let (provider, asks) = telling("old.dat", gate);
        let pool = ThumbPool::new(1, dir.path().to_owned(), provider);
        pool.want(1, vec![want(dir.path(), "old.dat", 0)]);
        assert_eq!(asks.recv_timeout(WAIT).ok().as_deref(), Some("old.dat"));
        // Another folder.
        pool.want(2, vec![want(dir.path(), "new.dat", 0)]);
        open.send(()).unwrap();
        let got = collect(&pool, 1);
        assert_eq!(got, [("new.dat".to_owned(), 2, Some((48, 24)))]);
        nothing_more(&pool);
    }

    #[test]
    fn a_provider_that_sleeps_fails_only_its_file_and_a_new_worker_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        // Released when the test ends: the stuck thread then ends too.
        let (_open, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let provider: ThumbProvider = Arc::new(move |path: &Path, _size: u32| {
            if name_of(path) == "slow.dat" {
                let _ = gate.lock().unwrap().recv_timeout(THUMB_TIMEOUT * 4);
            }
            made(16, 16)
        });
        let pool = ThumbPool::new(1, dir.path().to_owned(), provider);
        let asked = Instant::now();
        pool.want(
            1,
            vec![
                want(dir.path(), "slow.dat", 0),
                want(dir.path(), "next.dat", 1),
            ],
        );
        let got = collect(&pool, 2);
        assert_eq!(
            got,
            [
                ("slow.dat".to_owned(), 1, None),
                ("next.dat".to_owned(), 1, Some((16, 16))),
            ]
        );
        let took = asked.elapsed();
        assert!(took >= THUMB_TIMEOUT, "{took:?}");
        assert!(took < THUMB_TIMEOUT * 3, "{took:?}");
        // The file that stuck is not asked for again in this folder.
        pool.want(1, vec![want(dir.path(), "slow.dat", 0)]);
        nothing_more(&pool);
    }

    #[test]
    fn a_file_under_way_for_the_last_listing_is_still_delivered_in_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let (open, gate) = mpsc::channel();
        let (provider, asks) = telling("a.dat", gate);
        let pool = ThumbPool::new(1, dir.path().to_owned(), provider);
        pool.want(1, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(asks.recv_timeout(WAIT).ok().as_deref(), Some("a.dat"));
        // Refreshed (or sorted) while it is made: the same file is on screen in the new listing.
        pool.want(2, vec![want(dir.path(), "a.dat", 0)]);
        open.send(()).unwrap();
        assert_eq!(collect(&pool, 1), [("a.dat".to_owned(), 2, Some((48, 24)))]);
        nothing_more(&pool);
        // Made once: the new listing's task found it in memory.
        assert_eq!(asks.try_iter().count(), 0);
    }

    /// A provider that sticks on every file for as long as the returned sender lives, counting
    /// its calls.
    fn sticking() -> (ThumbProvider, mpsc::Sender<()>, Arc<AtomicUsize>) {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let provider: ThumbProvider = Arc::new(move |_: &Path, _: u32| {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = gate.lock().unwrap().recv_timeout(WAIT);
            made(8, 8)
        });
        (provider, release, calls)
    }

    #[test]
    fn a_file_that_timed_out_is_not_asked_for_again_in_the_next_listing() {
        let dir = tempfile::tempdir().unwrap();
        let (provider, _release, calls) = sticking();
        let pool = ThumbPool::with_limits(
            1,
            dir.path().to_owned(),
            provider,
            Duration::from_millis(100),
            8,
        );
        pool.want(1, vec![want(dir.path(), "slow.dat", 0)]);
        assert_eq!(collect(&pool, 1), [("slow.dat".to_owned(), 1, None)]);
        // A refresh is a new listing, but the file timed out a moment ago.
        pool.want(2, vec![want(dir.path(), "slow.dat", 0)]);
        nothing_more(&pool);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn too_many_stuck_threads_stop_the_workers_and_the_rest_fail_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (provider, release, calls) = sticking();
        let pool = ThumbPool::with_limits(
            1,
            dir.path().to_owned(),
            provider,
            Duration::from_millis(100),
            2,
        );
        let names = ["a.dat", "b.dat", "c.dat", "d.dat", "e.dat"];
        pool.want(
            1,
            names
                .iter()
                .enumerate()
                .map(|(rank, name)| want(dir.path(), name, rank))
                .collect(),
        );
        // Two files stick (a thread each); no worker is started for the rest, which fail.
        let got = collect(&pool, names.len());
        assert_eq!(got.len(), names.len(), "{got:?}");
        assert!(got.iter().all(|(_, _, size)| size.is_none()), "{got:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // When the stuck calls return, work goes on.
        drop(release);
        std::thread::sleep(QUIET);
        pool.want(2, vec![want(dir.path(), "f.dat", 0)]);
        let again = collect(&pool, 1);
        // Made, not shed.
        assert_eq!(again, [("f.dat".to_owned(), 2, Some((8, 8)))]);
    }

    #[test]
    fn a_picture_over_the_preview_s_size_cap_is_not_decoded() {
        let permit = || DECODING.acquire_within(WAIT);
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.png");
        File::create(&big)
            .unwrap()
            .set_len(preview::MAX_IMAGE_BYTES + 1)
            .unwrap();
        assert!(decoded(&big, preview::MAX_IMAGE_BYTES + 1, 96, &permit).is_none());
        let small = dir.path().join("small.png");
        RgbaImage::from_pixel(30, 30, image::Rgba([1, 2, 3, 255]))
            .save(&small)
            .unwrap();
        let len = fs::metadata(&small).unwrap().len();
        assert_eq!(
            decoded(&small, len, 96, &permit).map(|image| image.dimensions()),
            Some((30, 30))
        );
    }

    #[test]
    fn the_decodes_wait_for_a_permit() {
        static PERMITS: Permits = Permits {
            free: Mutex::new(1),
            freed: Condvar::new(),
        };
        let first = PERMITS.acquire_within(WAIT).unwrap();
        let (done, waited) = mpsc::channel();
        std::thread::spawn(move || {
            let _second = PERMITS.acquire_within(WAIT);
            let _ = done.send(());
        });
        assert!(waited.recv_timeout(QUIET).is_err(), "waits for the first");
        drop(first);
        assert!(waited.recv_timeout(WAIT).is_ok());
    }

    /// A pool whose decoder waits on `permits` for `permit_wait`, with the watchdog at `timeout`,
    /// and a PNG `pic.png` in `dir` that the provider (which has none) leaves to the decoder.
    fn decoding_pool(
        dir: &Path,
        permits: &'static Permits,
        timeout: Duration,
        permit_wait: Duration,
    ) -> ThumbPool {
        RgbaImage::from_pixel(30, 30, image::Rgba([1, 2, 3, 255]))
            .save(dir.join("pic.png"))
            .unwrap();
        let provider: ThumbProvider = Arc::new(|_: &Path, _: u32| ThumbOutcome::None);
        let mut pool = ThumbPool::with_limits(1, dir.to_owned(), provider, timeout, 8);
        let inner = Arc::get_mut(&mut pool.inner).unwrap();
        inner.permits = permits;
        inner.permit_wait = permit_wait;
        pool
    }

    fn want_pic(dir: &Path) -> ThumbWant {
        ThumbWant {
            path: dir.join("pic.png"),
            size: 48,
            mtime: None,
            len: fs::metadata(dir.join("pic.png")).unwrap().len(),
            visible_rank: 0,
        }
    }

    #[test]
    fn a_wait_for_a_decode_permit_is_not_timed_by_the_watchdog() {
        static PERMITS: Permits = Permits {
            free: Mutex::new(0),
            freed: Condvar::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let pool = decoding_pool(
            dir.path(),
            &PERMITS,
            Duration::from_millis(200),
            Duration::from_secs(30),
        );
        pool.want(1, vec![want_pic(dir.path())]);
        // Longer than the watchdog's timeout, with a permit short.
        std::thread::sleep(Duration::from_millis(900));
        assert!(pool.take_ready().is_empty());
        *PERMITS.free.lock().unwrap() += 1;
        PERMITS.freed.notify_one();
        assert_eq!(
            collect(&pool, 1),
            [("pic.png".to_owned(), 1, Some((30, 30)))]
        );
        let state = pool.inner.lock();
        assert!(state.timed_out.is_empty());
        assert_eq!(state.stuck, 0);
    }

    #[test]
    fn a_permit_wait_that_runs_out_fails_the_request_without_marking_it() {
        static PERMITS: Permits = Permits {
            free: Mutex::new(0),
            freed: Condvar::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let pool = decoding_pool(
            dir.path(),
            &PERMITS,
            Duration::from_secs(30),
            Duration::from_millis(200),
        );
        pool.want(1, vec![want_pic(dir.path())]);
        assert_eq!(collect(&pool, 1), [("pic.png".to_owned(), 1, None)]);
        {
            let state = pool.inner.lock();
            assert!(state.timed_out.is_empty());
            assert!(state.none.is_empty(), "a later ask in this listing retries");
            assert_eq!(state.stuck, 0);
            assert_eq!(state.alive, 1);
        }
        // A permit is free now: asked again, it is made.
        *PERMITS.free.lock().unwrap() += 1;
        PERMITS.freed.notify_one();
        pool.want(1, vec![want_pic(dir.path())]);
        assert_eq!(
            collect(&pool, 1),
            [("pic.png".to_owned(), 1, Some((30, 30)))]
        );
    }

    #[test]
    fn a_provider_that_panics_fails_only_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let provider: ThumbProvider = Arc::new(|path: &Path, _size: u32| {
            assert!(name_of(path) != "boom.dat", "a bug in the provider");
            made(8, 8)
        });
        let pool = ThumbPool::new(1, dir.path().to_owned(), provider);
        pool.want(
            1,
            vec![
                want(dir.path(), "boom.dat", 0),
                want(dir.path(), "fine.dat", 1),
            ],
        );
        assert_eq!(
            collect(&pool, 2),
            [
                ("boom.dat".to_owned(), 1, None),
                ("fine.dat".to_owned(), 1, Some((8, 8))),
            ]
        );
        // The worker goes on.
        pool.want(2, vec![want(dir.path(), "more.dat", 0)]);
        assert_eq!(
            collect(&pool, 1),
            [("more.dat".to_owned(), 2, Some((8, 8)))]
        );
    }

    /// A provider that counts its calls and makes a 20 × 10 picture.
    fn counting() -> (ThumbProvider, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let provider: ThumbProvider = Arc::new(move |_: &Path, _: u32| {
            counted.fetch_add(1, Ordering::SeqCst);
            made(20, 10)
        });
        (provider, calls)
    }

    #[test]
    fn a_thumbnail_in_the_disk_cache_does_not_ask_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let (provider, calls) = counting();
        let pool = ThumbPool::new(2, cache.clone(), provider);
        pool.want(1, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(collect(&pool, 1), [("a.dat".to_owned(), 1, Some((20, 10)))]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let written = cache_file(&cache, &want(dir.path(), "a.dat", 0));
        assert!(written.is_file(), "{written:?}");
        drop(pool);

        // A new run: nothing in memory, the file on disk.
        let (provider, calls) = counting();
        let pool = ThumbPool::new(2, cache.clone(), provider);
        pool.want(5, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(collect(&pool, 1), [("a.dat".to_owned(), 5, Some((20, 10)))]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Another size is another thumbnail.
        let mut bigger = want(dir.path(), "a.dat", 0);
        bigger.size = 96;
        pool.want(5, vec![bigger]);
        assert_eq!(collect(&pool, 1).len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_thumbnail_in_memory_comes_again_without_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let (provider, calls) = counting();
        let pool = ThumbPool::new(1, dir.path().join("cache"), provider);
        pool.want(1, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(collect(&pool, 1).len(), 1);
        // Back in the folder (a new listing).
        pool.want(2, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(collect(&pool, 1), [("a.dat".to_owned(), 2, Some((20, 10)))]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn no_thumbnail_is_remembered_for_the_folder_only() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let provider: ThumbProvider = Arc::new(move |_: &Path, _: u32| {
            counted.fetch_add(1, Ordering::SeqCst);
            ThumbOutcome::None
        });
        let cache = dir.path().join("cache");
        let pool = ThumbPool::new(1, cache.clone(), provider);
        pool.want(1, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(collect(&pool, 1), [("a.dat".to_owned(), 1, None)]);
        // Not asked again while the folder shows…
        pool.want(1, vec![want(dir.path(), "a.dat", 0)]);
        nothing_more(&pool);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // …but again in its next listing: the system's "none" may pass (Windows' E_FAIL), so
        // nothing of it goes to the disk.
        pool.want(2, vec![want(dir.path(), "a.dat", 0)]);
        assert_eq!(collect(&pool, 1), [("a.dat".to_owned(), 2, None)]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!cache_file(&cache, &want(dir.path(), "a.dat", 0)).exists());
    }

    #[test]
    fn a_picture_the_system_does_not_make_is_decoded_by_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let picture = RgbaImage::from_pixel(300, 150, image::Rgba([200, 10, 10, 255]));
        let file = dir.path().join("red.png");
        picture.save(&file).unwrap();
        let provider: ThumbProvider = Arc::new(|_: &Path, _: u32| ThumbOutcome::None);
        let pool = ThumbPool::new(1, dir.path().join("cache"), provider);
        let mut item = want(dir.path(), "red.png", 0);
        item.size = 96;
        pool.want(1, vec![item]);
        assert_eq!(
            collect(&pool, 1),
            [("red.png".to_owned(), 1, Some((96, 48)))]
        );
    }

    #[test]
    fn the_cache_key_names_the_path_the_length_the_time_and_the_size() {
        let path = Path::new("/a/b.png");
        let time = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let key = cache_key(path, 10, time, 48);
        assert_eq!(key.len(), 32);
        assert!(
            key.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(key, cache_key(path, 10, time, 48), "the same each time");
        for other in [
            cache_key(Path::new("/a/c.png"), 10, time, 48),
            cache_key(path, 11, time, 48),
            cache_key(path, 10, time.map(|t| t + Duration::from_micros(1)), 48),
            cache_key(path, 10, None, 48),
            cache_key(path, 10, time, 96),
        ] {
            assert_ne!(key, other);
        }
    }

    #[test]
    fn the_prune_plan_takes_the_oldest_until_the_rest_fits() {
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        let entries = vec![
            (PathBuf::from("new"), 40, at(300)),
            (PathBuf::from("old"), 30, at(100)),
            (PathBuf::from("mid"), 30, at(200)),
        ];
        assert_eq!(prune_plan(&entries, 100), Vec::<PathBuf>::new());
        assert_eq!(prune_plan(&entries, 99), [PathBuf::from("old")]);
        assert_eq!(prune_plan(&entries, 70), [PathBuf::from("old")]);
        assert_eq!(
            prune_plan(&entries, 69),
            [PathBuf::from("old"), PathBuf::from("mid")]
        );
        assert_eq!(prune_plan(&entries, 0).len(), 3);
        assert_eq!(prune_plan(&[], 0), Vec::<PathBuf>::new());
    }

    #[test]
    fn the_cache_keeps_to_its_cap_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("48");
        fs::create_dir_all(&folder).unwrap();
        for (name, len) in [("a.png", 600), ("b.png", 600)] {
            fs::write(folder.join(name), vec![0; len]).unwrap();
        }
        prune(dir.path(), 700);
        let left: Vec<_> = fs::read_dir(&folder).unwrap().collect();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn the_schedule_follows_the_screen() {
        let dir = Path::new("d");
        let wants = vec![want(dir, "c", 2), want(dir, "a", 0), want(dir, "b", 1)];
        let order: Vec<String> = schedule(&wants)
            .iter()
            .map(|want| name_of(&want.path))
            .collect();
        assert_eq!(order, ["a", "b", "c"]);
    }

    #[test]
    fn the_workers_are_two_to_four() {
        assert!((DEFAULT_WORKERS..=MAX_WORKERS).contains(&worker_count()));
        assert_eq!(THUMB_TIMEOUT, Duration::from_secs(5));
        assert_eq!(CACHE_CAP, 256 * 1024 * 1024);
    }

    #[test]
    fn the_visible_files_are_wanted_by_their_place_on_screen() {
        use rf_core::{Attributes, Entry, EntryKind};
        let mut entries = vec![
            Entry::new("folder", EntryKind::Dir),
            Entry::new("a.png", EntryKind::File),
            Entry::new("cloud.png", EntryKind::File),
            Entry::new("b.png", EntryKind::File),
            Entry::new("c.png", EntryKind::File),
        ];
        entries[2].attributes = Attributes::OFFLINE;
        entries[3].size = 7;
        let wants = visible_wants(&entries, Path::new("/x"), 0, 3, 96);
        // Folders keep their icon, a file online only is not downloaded for its picture, and
        // only the rows on screen are asked for.
        let names: Vec<(String, usize, u64)> = wants
            .iter()
            .map(|want| (name_of(&want.path), want.visible_rank, want.len))
            .collect();
        assert_eq!(
            names,
            [("a.png".to_owned(), 1, 0), ("b.png".to_owned(), 3, 7)]
        );
        assert!(wants.iter().all(|want| want.size == 96));
        assert_eq!(visible_wants(&entries, Path::new("/x"), 3, 99, 96).len(), 2);
        assert_eq!(visible_wants(&entries, Path::new("/x"), 9, 99, 96), []);
        // None on screen.
        assert_eq!(visible_wants(&entries, Path::new("/x"), 1, 0, 96), []);
    }
}
