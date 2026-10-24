//! Diagnostics: cheap measures of how the program behaves (frame times,
//! freezes, the slowest operations, canvas rendering, background jobs) and
//! the warnings and errors it reports, kept in memory and in a log file.
//! Help › Diagnostics shows them and copies them as a report (see
//! `app::diagnostics`).
//!
//! Measuring costs a lock and two clock reads per frame or operation:
//! nothing is computed until the window or the report asks for it.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::i18n::Message;

/// A frame longer than this is a freeze.
pub const FREEZE: Duration = Duration::from_millis(100);
/// Frame time statistics are kept per second, for this many seconds.
const SECONDS: u64 = 300;
/// Operations kept by name, at most (the least recent go first).
const MAX_OPERATIONS: usize = 256;
/// The last operations, kept to tell what a freeze was doing.
const RECENT_OPERATIONS: usize = 512;
const MAX_FREEZES: usize = 200;
/// Frames that composited canvas tiles, kept for a minute at most.
const MAX_TILE_FRAMES: usize = 4096;
const LOG_CAPACITY: usize = 300;
/// The log file is set aside (one older file kept) beyond this size.
const LOG_FILE_MAX: u64 = 1 << 20;

static START: LazyLock<Instant> = LazyLock::new(Instant::now);
static MEASURES: LazyLock<Mutex<Measures>> = LazyLock::new(|| Mutex::new(Measures::new(*START)));
static LOG: LazyLock<Mutex<Log>> = LazyLock::new(|| Mutex::new(Log::default()));
/// A background job running (see [`job`]): its key and name.
type Running = (u64, Cow<'static, str>);
static JOBS: LazyLock<Mutex<Vec<Running>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the clock of the session (its uptime).
pub fn start() {
    LazyLock::force(&START);
}

/// Since the program started.
pub fn uptime() -> Duration {
    START.elapsed()
}

/// One operation's statistics.
#[derive(Clone, Debug)]
pub struct OpStats {
    pub count: u32,
    pub total: Duration,
    pub max: Duration,
    pub last: Duration,
    /// When it last ended.
    pub last_at: Instant,
    /// Computed on another thread (it doesn't freeze the window by itself).
    pub background: bool,
}

/// A frame longer than [`FREEZE`], and what was going on.
#[derive(Clone, Debug)]
pub struct Freeze {
    pub at: Instant,
    pub took: Duration,
    pub context: Doing,
}

/// What the window was doing during a freeze, as names the window's code
/// shows in the interface's language (and the report in English).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doing {
    /// The active tool (its `Debug` name).
    pub tool: String,
    /// The open dialog: the key of its name.
    pub dialog: Option<&'static str>,
    pub stroke: bool,
    pub pointer_down: bool,
    /// The frame's longest operations (their recorded names).
    pub ran: Vec<(String, Duration)>,
    /// Background jobs running (their recorded names).
    pub jobs: Vec<String>,
}

/// Frame times over a period.
#[derive(Clone, Copy, Default, Debug)]
pub struct FrameStats {
    pub frames: u32,
    pub average: Duration,
    pub max: Duration,
}

/// Canvas tiles composited over the last minute.
#[derive(Clone, Copy, Default, Debug)]
pub struct TileStats {
    /// In the last frame that composited any.
    pub last: u32,
    pub max: u32,
    /// Frames that stopped at the time budget, tiles left for later.
    pub over_budget: u32,
    /// Level of reduction shown (0: full size, 1: half…).
    pub level: u32,
}

struct Second {
    index: u64,
    frames: u32,
    total: Duration,
    max: Duration,
}

struct RecentOp {
    at: Instant,
    /// The thread that ran it (a frame's operations are its thread's).
    thread: std::thread::ThreadId,
    name: Cow<'static, str>,
    took: Duration,
    background: bool,
}

/// What is measured, from `origin` (or since the last [`Measures::clear`]).
pub struct Measures {
    origin: Instant,
    since: Instant,
    ops: HashMap<Cow<'static, str>, OpStats>,
    recent: VecDeque<RecentOp>,
    seconds: VecDeque<Second>,
    freezes: VecDeque<Freeze>,
    /// (when, tiles, stopped at the budget).
    tiles: VecDeque<(Instant, u32, bool)>,
    level: u32,
}

impl Measures {
    pub fn new(origin: Instant) -> Self {
        Self {
            origin,
            since: origin,
            ops: HashMap::new(),
            recent: VecDeque::new(),
            seconds: VecDeque::new(),
            freezes: VecDeque::new(),
            tiles: VecDeque::new(),
            level: 0,
        }
    }

    /// Forgets everything measured.
    pub fn clear(&mut self, now: Instant) {
        *self = Self {
            since: now,
            level: self.level,
            ..Self::new(self.origin)
        };
    }

    /// When the measures started (the program, or the last clear).
    pub fn since(&self) -> Instant {
        self.since
    }

    /// An operation that took `took`, ending at `at`.
    pub fn record(&mut self, name: Cow<'static, str>, took: Duration, at: Instant, bg: bool) {
        if !self.ops.contains_key(&name) && self.ops.len() >= MAX_OPERATIONS {
            let oldest = self
                .ops
                .iter()
                .min_by_key(|(_, s)| s.last_at)
                .map(|(n, _)| n.clone());
            if let Some(oldest) = oldest {
                self.ops.remove(&oldest);
            }
        }
        let stats = self.ops.entry(name.clone()).or_insert(OpStats {
            count: 0,
            total: Duration::ZERO,
            max: Duration::ZERO,
            last: Duration::ZERO,
            last_at: at,
            background: bg,
        });
        stats.count += 1;
        stats.total += took;
        stats.max = stats.max.max(took);
        stats.last = took;
        stats.last_at = at;
        if self.recent.len() >= RECENT_OPERATIONS {
            self.recent.pop_front();
        }
        self.recent.push_back(RecentOp {
            at,
            thread: std::thread::current().id(),
            name,
            took,
            background: bg,
        });
    }

    /// A frame that started at `start` and took `took`. Returns whether it
    /// is a freeze (see [`Self::freeze`]).
    pub fn frame(&mut self, start: Instant, took: Duration) -> bool {
        let index = start.saturating_duration_since(self.origin).as_secs();
        match self.seconds.back_mut() {
            Some(s) if s.index == index => {
                s.frames += 1;
                s.total += took;
                s.max = s.max.max(took);
            }
            _ => self.seconds.push_back(Second {
                index,
                frames: 1,
                total: took,
                max: took,
            }),
        }
        while self
            .seconds
            .front()
            .is_some_and(|s| s.index + SECONDS <= index)
        {
            self.seconds.pop_front();
        }
        took > FREEZE
    }

    /// A freeze, and what was going on.
    pub fn freeze(&mut self, at: Instant, took: Duration, context: Doing) {
        if self.freezes.len() >= MAX_FREEZES {
            self.freezes.pop_front();
        }
        self.freezes.push_back(Freeze { at, took, context });
        let horizon = Duration::from_secs(SECONDS);
        while self
            .freezes
            .front()
            .is_some_and(|f| at.saturating_duration_since(f.at) > horizon)
        {
            self.freezes.pop_front();
        }
    }

    /// The operations of this thread (the window's) that ended at or
    /// after `start`, the longest first.
    pub fn ops_since(&self, start: Instant) -> Vec<(String, Duration)> {
        let thread = std::thread::current().id();
        let mut ops: Vec<(String, Duration)> = self
            .recent
            .iter()
            .filter(|o| o.at >= start && !o.background && o.thread == thread)
            .map(|o| (o.name.to_string(), o.took))
            .collect();
        ops.sort_by_key(|o| std::cmp::Reverse(o.1));
        ops
    }

    /// Canvas tiles composited in a frame, at `level`; `over`: it stopped at
    /// its time budget.
    pub fn tiles(&mut self, at: Instant, tiles: u32, over: bool, level: u32) {
        self.level = level;
        if self.tiles.len() >= MAX_TILE_FRAMES {
            self.tiles.pop_front();
        }
        self.tiles.push_back((at, tiles, over));
        let minute = Duration::from_secs(60);
        while self
            .tiles
            .front()
            .is_some_and(|t| at.saturating_duration_since(t.0) > minute)
        {
            self.tiles.pop_front();
        }
    }

    /// Frame times over the last `period` before `now`.
    pub fn frame_stats(&self, now: Instant, period: Duration) -> FrameStats {
        let index = now.saturating_duration_since(self.origin).as_secs();
        let first = index.saturating_sub(period.as_secs().saturating_sub(1));
        let mut stats = FrameStats::default();
        let mut total = Duration::ZERO;
        for s in self.seconds.iter().filter(|s| s.index >= first) {
            stats.frames += s.frames;
            total += s.total;
            stats.max = stats.max.max(s.max);
        }
        if stats.frames > 0 {
            stats.average = total / stats.frames;
        }
        stats
    }

    /// The freezes over the last `period` before `now`, the latest last.
    pub fn freezes(&self, now: Instant, period: Duration) -> Vec<&Freeze> {
        self.freezes
            .iter()
            .filter(|f| now.saturating_duration_since(f.at) <= period)
            .collect()
    }

    /// Every operation measured, by name.
    pub fn operations(&self) -> Vec<(&str, &OpStats)> {
        self.ops.iter().map(|(n, s)| (n.as_ref(), s)).collect()
    }

    /// The canvas tiles composited over the last minute before `now`.
    pub fn tile_stats(&self, now: Instant) -> TileStats {
        let minute = Duration::from_secs(60);
        let recent = self
            .tiles
            .iter()
            .filter(|t| now.saturating_duration_since(t.0) <= minute);
        let mut stats = TileStats {
            level: self.level,
            last: self.tiles.back().map_or(0, |t| t.1),
            ..Default::default()
        };
        for &(_, tiles, over) in recent {
            stats.max = stats.max.max(tiles);
            stats.over_budget += u32::from(over);
        }
        stats
    }
}

/// Reads the measures.
pub fn measures<R>(f: impl FnOnce(&Measures) -> R) -> R {
    f(&lock(&MEASURES))
}

/// Help › Diagnostics › Clear Measures.
pub fn clear() {
    lock(&MEASURES).clear(Instant::now());
}

/// An operation of the window's thread that took `took` (just ended).
pub fn record(name: impl Into<Cow<'static, str>>, took: Duration) {
    lock(&MEASURES).record(name.into(), took, Instant::now(), false);
}

/// A frame of the window that started at `start` and took `took`: whether
/// it is a freeze.
pub fn frame(start: Instant, took: Duration) -> bool {
    lock(&MEASURES).frame(start, took)
}

pub fn freeze(took: Duration, context: Doing) {
    lock(&MEASURES).freeze(Instant::now(), took, context);
}

/// The window thread's operations that ended at or after `start`.
pub fn ops_since(start: Instant) -> Vec<(String, Duration)> {
    lock(&MEASURES).ops_since(start)
}

pub fn tiles(tiles: u32, over_budget: bool, level: u32) {
    lock(&MEASURES).tiles(Instant::now(), tiles, over_budget, level);
}

/// Measures an operation of the window's thread until it is dropped.
#[must_use = "measures until dropped"]
pub struct Timer {
    name: Option<Cow<'static, str>>,
    start: Instant,
    /// A background job: its key in [`JOBS`].
    job: Option<u64>,
}

impl Drop for Timer {
    fn drop(&mut self) {
        let Some(name) = self.name.take() else { return };
        let took = self.start.elapsed();
        if let Some(key) = self.job {
            lock(&JOBS).retain(|(k, _)| *k != key);
        }
        lock(&MEASURES).record(name, took, Instant::now(), self.job.is_some());
    }
}

/// Measures the window thread's operation `name` until the result is
/// dropped.
pub fn timer(name: impl Into<Cow<'static, str>>) -> Timer {
    Timer {
        name: Some(name.into()),
        start: Instant::now(),
        job: None,
    }
}

/// Measures a background job until the result is dropped (at the end of
/// its thread); meanwhile it is listed as running.
pub fn job(name: impl Into<Cow<'static, str>>) -> Timer {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let key = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = name.into();
    lock(&JOBS).push((key, name.clone()));
    Timer {
        name: Some(name),
        start: Instant::now(),
        job: Some(key),
    }
}

/// The background jobs running now (see [`job`]).
pub fn running_jobs() -> Vec<String> {
    lock(&JOBS).iter().map(|(_, n)| n.to_string()).collect()
}

// ---- Log ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Warning,
    Error,
    /// A panic (caught or not), with where it happened.
    Panic,
}

impl Level {
    pub fn code(self) -> &'static str {
        match self {
            Level::Warning => "WARN",
            Level::Error => "ERROR",
            Level::Panic => "PANIC",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub at: Instant,
    pub wall: SystemTime,
    pub level: Level,
    /// In the interface's language (the window's).
    pub message: String,
    /// In English (the log file's and the report's).
    pub english: String,
}

#[derive(Default)]
struct Log {
    entries: VecDeque<LogEntry>,
    /// Counted since the start, beyond what the ring keeps.
    warnings: u32,
    errors: u32,
    /// The file got the session's first line.
    file_started: bool,
}

/// Something that went wrong without stopping anything (a text of the
/// interface, or a `tm!` message with its English).
pub fn warn(message: impl Into<Message>) {
    log(Level::Warning, &message.into());
}

/// A failed operation (shown to the user as one).
pub fn error(message: impl Into<Message>) {
    log(Level::Error, &message.into());
}

fn log(level: Level, message: &Message) {
    use std::sync::TryLockError;
    // A panic while this very thread holds the log (writing it) would wait
    // forever: a panicking thread tries for a moment, then gives up.
    let mut tries = 0;
    let mut log = loop {
        match LOG.try_lock() {
            Ok(log) => break log,
            Err(TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(TryLockError::WouldBlock) if !std::thread::panicking() => break lock(&LOG),
            Err(TryLockError::WouldBlock) if tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(TryLockError::WouldBlock) => return,
        }
    };
    add(&mut log, level, message);
}

fn add(log: &mut Log, level: Level, message: &Message) {
    let entry = LogEntry {
        at: Instant::now(),
        wall: SystemTime::now(),
        level,
        message: scrub(message.shown.trim()),
        english: scrub(message.english.trim()),
    };
    match level {
        Level::Warning => log.warnings += 1,
        Level::Error | Level::Panic => log.errors += 1,
    }
    write_file(log, &entry);
    if log.entries.len() >= LOG_CAPACITY {
        log.entries.pop_front();
    }
    log.entries.push_back(entry);
}

/// The messages kept (the latest last), and how many warnings and errors
/// there were since the start.
pub fn log_entries() -> (Vec<LogEntry>, u32, u32) {
    let log = lock(&LOG);
    (
        log.entries.iter().cloned().collect(),
        log.warnings,
        log.errors,
    )
}

/// Where the log files are.
pub fn log_dir() -> PathBuf {
    crate::config::dir().join("logs")
}

pub fn log_file() -> PathBuf {
    log_dir().join("atelier.log")
}

/// Appends `entry` to the log file, set aside once too large. Never in
/// tests.
fn write_file(log: &mut Log, entry: &LogEntry) {
    if cfg!(test) {
        return;
    }
    let path = log_file();
    let _ = std::fs::create_dir_all(log_dir());
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_FILE_MAX) {
        let _ = std::fs::rename(&path, log_dir().join("atelier.old.log"));
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if !log.file_started {
        log.file_started = true;
        let _ = writeln!(
            file,
            "--- {} Atelier {} ({}, {})",
            timestamp(entry.wall),
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    let _ = writeln!(
        file,
        "{} {} {}",
        timestamp(entry.wall),
        entry.level.code(),
        entry.english
    );
}

/// "2026-10-23 14:02:11Z" (UTC).
pub fn timestamp(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = crate::actions::civil_date((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// Logs every panic (any thread, caught or not) with where it happened,
/// then lets the default hook print it.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log(Level::Panic, &panic_message(info).into());
        previous(info);
    }));
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_owned());
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    match info.location() {
        Some(l) => format!(
            "{message} (thread {thread}, {}:{}:{})",
            l.file(),
            l.line(),
            l.column()
        ),
        None => format!("{message} (thread {thread})"),
    }
}

// ---- Privacy ----

/// The user's home folder, as written in paths.
fn home() -> Option<String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let home = home
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .to_owned();
    (home.len() > 1).then_some(home)
}

/// `text` without personal data: the home folder becomes `~`, and paths
/// keep only their file name.
pub fn scrub(text: &str) -> String {
    let mut text = text.to_owned();
    if let Some(home) = home() {
        text = text.replace(&home, "~");
        let other = if home.contains('\\') {
            home.replace('\\', "/")
        } else {
            home.replace('/', "\\")
        };
        text = text.replace(&other, "~");
    }
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        let mut first_word = true;
        for word in line.split(' ') {
            if !first_word {
                out.push(' ');
            }
            first_word = false;
            out.push_str(&scrub_word(word));
        }
    }
    out
}

/// A word that is a path (absolute, or from `~`) keeps its file name only.
fn scrub_word(word: &str) -> Cow<'_, str> {
    let start = word
        .find(|c: char| !matches!(c, '(' | '"' | '\'' | '«' | '[' | '<' | '`'))
        .unwrap_or(word.len());
    let end = word
        .rfind(|c: char| {
            !matches!(
                c,
                ')' | '"' | '\'' | '»' | ']' | '>' | '`' | ',' | ';' | ':' | '.' | '!' | '?'
            )
        })
        .map_or(start, |i| {
            i + word[i..].chars().next().map_or(1, char::len_utf8)
        });
    if start >= end {
        return Cow::Borrowed(word);
    }
    let core = &word[start..end];
    let bytes = core.as_bytes();
    let drive = bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let rooted = (core.starts_with('/') || core.starts_with("\\\\")) && core.len() > 1;
    let home = core.starts_with("~/") || core.starts_with("~\\");
    if !(drive || rooted || home) {
        return Cow::Borrowed(word);
    }
    let name = core
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("");
    Cow::Owned(format!("{}{name}{}", &word[..start], &word[end..]))
}

// ---- The process ----

/// Memory of the process: what is in RAM, and what is its own (not
/// shared with other processes, e.g. libraries), when the system says.
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub resident: u64,
    pub private: Option<u64>,
}

/// The process's memory: /proc on Linux, GetProcessMemoryInfo on Windows,
/// proc_pidinfo on macOS.
pub fn process_memory() -> Option<Memory> {
    platform::memory()
}

/// The process's threads, when the system says.
pub fn thread_count() -> Option<u32> {
    platform::threads()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Memory;

    /// A "Name:   1234 kB" line of /proc/self/status, in bytes (or a count).
    fn status_field(status: &str, field: &str) -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(field))?;
        let mut words = line[field.len()..].split_whitespace();
        let value: u64 = words.next()?.parse().ok()?;
        Some(match words.next() {
            Some("kB") => value * 1024,
            _ => value,
        })
    }

    pub fn memory() -> Option<Memory> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        Some(Memory {
            resident: status_field(&status, "VmRSS:")?,
            // Anonymous memory: the heap and the stacks (what isn't
            // mapped from a file, like the libraries).
            private: status_field(&status, "RssAnon:"),
        })
    }

    pub fn threads() -> Option<u32> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status_field(&status, "Threads:").map(|n| n as u32)
    }
}

#[cfg(windows)]
mod platform {
    use super::Memory;

    /// PROCESS_MEMORY_COUNTERS_EX.
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut core::ffi::c_void,
            counters: *mut Counters,
            cb: u32,
        ) -> i32;
    }

    pub fn memory() -> Option<Memory> {
        let mut c = Counters {
            cb: size_of::<Counters>() as u32,
            ..Default::default()
        };
        // SAFETY: the pseudo-handle of the current process needs no
        // closing; the call fills the structure whose size it is given.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        (ok != 0).then_some(Memory {
            resident: c.working_set_size as u64,
            private: Some(c.private_usage as u64),
        })
    }

    pub fn threads() -> Option<u32> {
        None
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Memory;

    fn task_info() -> Option<libc::proc_taskinfo> {
        // SAFETY: zeroed is a valid proc_taskinfo (plain integers);
        // proc_pidinfo writes at most the size it is given.
        unsafe {
            let mut info: libc::proc_taskinfo = std::mem::zeroed();
            let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
            let written = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            );
            (written == size).then_some(info)
        }
    }

    pub fn memory() -> Option<Memory> {
        task_info().map(|i| Memory {
            resident: i.pti_resident_size,
            private: None,
        })
    }

    pub fn threads() -> Option<u32> {
        task_info().map(|i| i.pti_threadnum as u32)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::Memory;

    pub fn memory() -> Option<Memory> {
        None
    }

    pub fn threads() -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_resets_the_counters() {
        let origin = Instant::now();
        let mut m = Measures::new(origin);
        let at = origin + Duration::from_secs(2);
        m.record("Open".into(), Duration::from_millis(40), at, false);
        m.record("Open".into(), Duration::from_millis(60), at, false);
        assert!(m.frame(at, Duration::from_millis(150)));
        let brush = Doing {
            tool: "Brush".into(),
            ..Doing::default()
        };
        m.freeze(at, Duration::from_millis(150), brush);
        m.tiles(at, 12, true, 1);
        let ops = m.operations();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].1.count, 2);
        assert_eq!(ops[0].1.max, Duration::from_millis(60));
        assert_eq!(m.frame_stats(at, Duration::from_secs(60)).frames, 1);
        assert_eq!(m.tile_stats(at).over_budget, 1);
        m.clear(at);
        assert!(m.operations().is_empty());
        assert!(m.ops_since(origin).is_empty());
        assert!(m.freezes(at, Duration::from_secs(300)).is_empty());
        assert_eq!(m.frame_stats(at, Duration::from_secs(60)).frames, 0);
        let tiles = m.tile_stats(at);
        assert_eq!((tiles.max, tiles.over_budget, tiles.last), (0, 0, 0));
        assert_eq!(m.since(), at);
    }

    #[test]
    fn frame_times_cover_their_period() {
        let origin = Instant::now();
        let mut m = Measures::new(origin);
        let ms = Duration::from_millis;
        assert!(!m.frame(origin, ms(10)));
        assert!(!m.frame(origin + ms(500), ms(30)));
        // Two minutes later: the first second is out of the last minute.
        let later = origin + Duration::from_secs(120);
        assert!(m.frame(later, ms(200)));
        let minute = m.frame_stats(later, Duration::from_secs(60));
        assert_eq!((minute.frames, minute.max), (1, ms(200)));
        let five = m.frame_stats(later, Duration::from_secs(300));
        assert_eq!(five.frames, 3);
        assert_eq!(five.average, ms(80));
        // Old freezes leave the last minute but stay for five.
        m.freeze(origin, ms(150), Doing::default());
        m.freeze(later, ms(200), Doing::default());
        assert_eq!(m.freezes(later, Duration::from_secs(60)).len(), 1);
        assert_eq!(m.freezes(later, Duration::from_secs(300)).len(), 2);
    }

    /// What a freeze was doing: the operations of its frame, not the
    /// background ones.
    #[test]
    fn ops_of_a_frame_leave_out_background_jobs() {
        let origin = Instant::now();
        let mut m = Measures::new(origin);
        let ms = Duration::from_millis;
        m.record("Before".into(), ms(5), origin, false);
        let frame = origin + ms(100);
        m.record(
            "Filter(GaussianBlur)".into(),
            ms(300),
            frame + ms(300),
            false,
        );
        m.record("Undo".into(), ms(20), frame + ms(320), false);
        m.record("Selection job".into(), ms(900), frame + ms(330), true);
        let ops = m.ops_since(frame);
        let names: Vec<&str> = ops.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Filter(GaussianBlur)", "Undo"]);
    }

    #[test]
    fn jobs_are_listed_while_they_run() {
        let job = job("Test job 7a1");
        assert!(running_jobs().iter().any(|j| j == "Test job 7a1"));
        drop(job);
        assert!(!running_jobs().iter().any(|j| j == "Test job 7a1"));
        let ops = measures(|m| {
            m.operations()
                .into_iter()
                .filter(|(n, _)| *n == "Test job 7a1")
                .map(|(_, s)| s.background)
                .collect::<Vec<_>>()
        });
        assert_eq!(ops, [true]);
    }

    #[test]
    fn paths_keep_their_file_name_only() {
        assert_eq!(
            scrub("Can't open /srv/photos/2024/plage.jpg: denied"),
            "Can't open plage.jpg: denied"
        );
        assert_eq!(
            scrub("Impossible d'ouvrir « C:\\Users\\Jo\\Pictures\\a.png »."),
            "Impossible d'ouvrir « a.png »."
        );
        assert_eq!(scrub("(~/x/y/model.onnx)"), "(model.onnx)");
        // Not paths.
        assert_eq!(scrub("1/2 w/h 50 %"), "1/2 w/h 50 %");
        if let Some(home) = home() {
            let text = scrub(&format!("in {home} and {home}/dir/f.txt"));
            assert!(!text.contains(&home), "{text}");
            assert_eq!(text, "in ~ and f.txt");
        }
    }

    #[test]
    fn panics_are_logged_with_their_place() {
        install_panic_hook();
        let result = crate::ai::catch_panic::<()>(|| panic!("diag test panic 41"));
        assert!(result.is_err());
        let (entries, _, errors) = log_entries();
        let entry = entries
            .iter()
            .find(|e| e.message.contains("diag test panic 41"))
            .expect("logged");
        assert_eq!(entry.level, Level::Panic);
        assert!(entry.message.contains("diag.rs"), "{}", entry.message);
        assert!(errors >= 1);
    }

    #[test]
    fn timestamps_are_utc() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400 * 365 + 3661);
        assert_eq!(timestamp(at), "1971-01-01 01:01:01Z");
    }

    #[test]
    fn the_process_memory_is_known() {
        if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            let m = process_memory().expect("memory");
            assert!(m.resident > 1 << 20);
        }
    }
}
