//! The AI models Atelier can download and run locally: what they are, where
//! they live, download (checked against size and digest), deletion.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::runtime::Device;
use crate::i18n::t;

/// A file of a model.
pub struct ModelFile {
    /// Name on disk (ONNX external data must keep the name the model refers to).
    pub name: &'static str,
    pub url: &'static str,
    /// Expected size and digest: a truncated or altered file is rejected.
    pub size: u64,
    pub sha256: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Model {
    /// BiRefNet (lite): Select Subject, Remove Background.
    Subject,
    /// SAM 2.1 (tiny): Object Selection tool.
    Objects,
    /// LaMa: Content-Aware Fill, Spot Healing Brush on large areas.
    Inpaint,
}

const SUBJECT_FILES: &[ModelFile] = &[ModelFile {
    name: "birefnet-general-lite.onnx",
    url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/\
          BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
    size: 224_005_088,
    sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
}];

/// SAM 2.1 files, at a fixed revision of the repository.
macro_rules! sam_url {
    ($file:literal) => {
        concat!(
            "https://huggingface.co/onnx-community/sam2.1-hiera-tiny-ONNX/resolve/",
            "814a066640debee5a91e70aa401fb8e17e030503/onnx/",
            $file
        )
    };
}

const OBJECTS_FILES: &[ModelFile] = &[
    ModelFile {
        name: "vision_encoder.onnx",
        url: sam_url!("vision_encoder.onnx"),
        size: 354_238,
        sha256: "4f30aacd3aaefbca81a0b7fe4c1fc96345570ea0a6f80ced599493d1b3be2e8c",
    },
    ModelFile {
        name: "vision_encoder.onnx_data",
        url: sam_url!("vision_encoder.onnx_data"),
        size: 134_084_864,
        sha256: "e83df9866a5afe68ea7f0f721f18f65137fc3acbf0da1c74e946d363e09c69cc",
    },
    ModelFile {
        name: "prompt_encoder_mask_decoder.onnx",
        url: sam_url!("prompt_encoder_mask_decoder.onnx"),
        size: 213_114,
        sha256: "874414704c5d686db7d206a35f6e15d26563d50c8c4468fccc6739bd7e491dcf",
    },
    ModelFile {
        name: "prompt_encoder_mask_decoder.onnx_data",
        url: sam_url!("prompt_encoder_mask_decoder.onnx_data"),
        size: 20_958_208,
        sha256: "e9874d900dd4134ed60eab1e97910327c2419e0b2954485d8fd6e7f1a1470f47",
    },
];

/// LaMa, as converted by OpenCV, at a fixed revision of the repository.
const INPAINT_FILES: &[ModelFile] = &[ModelFile {
    name: "inpainting_lama_2025jan.onnx",
    url: "https://huggingface.co/opencv/inpainting_lama/resolve/\
          aee6d22f0a13e5e35af1c9a1c3afd62841fc6f3f/inpainting_lama_2025jan.onnx",
    size: 92_591_623,
    sha256: "7df918ac3921d3daf0aae1d219776cf0dc4e4935f035af81841b40adcf74fdf2",
}];

impl Model {
    pub const ALL: [Model; 3] = [Model::Subject, Model::Objects, Model::Inpaint];

    /// Name of the model itself (not translated).
    pub fn name(self) -> &'static str {
        match self {
            Model::Subject => "BiRefNet (lite)",
            Model::Objects => "SAM 2.1 (tiny)",
            Model::Inpaint => "LaMa",
        }
    }

    /// What it is used for in Atelier.
    pub fn purpose(self) -> String {
        match self {
            Model::Subject => t!("ai-model-subject-purpose"),
            Model::Objects => t!("ai-model-objects-purpose"),
            Model::Inpaint => t!("retouch-model-purpose"),
        }
    }

    pub fn license(self) -> &'static str {
        match self {
            Model::Subject => "MIT",
            Model::Objects | Model::Inpaint => "Apache 2.0",
        }
    }

    pub fn files(self) -> &'static [ModelFile] {
        match self {
            Model::Subject => SUBJECT_FILES,
            Model::Objects => OBJECTS_FILES,
            Model::Inpaint => INPAINT_FILES,
        }
    }

    /// Total download size, in bytes.
    pub fn size(self) -> u64 {
        self.files().iter().map(|f| f.size).sum()
    }

    /// Folder of the model's files. BiRefNet stays at the root, where
    /// earlier versions put it.
    fn dir(self) -> PathBuf {
        match self {
            Model::Subject => root(),
            Model::Objects => root().join("sam2.1-hiera-tiny"),
            Model::Inpaint => root().join("lama"),
        }
    }

    /// Path of the model file named `name`.
    pub fn path(self, name: &str) -> PathBuf {
        self.dir().join(name)
    }

    /// Are all its files on the computer, complete?
    pub fn ready(self) -> bool {
        self.files()
            .iter()
            .all(|f| std::fs::metadata(self.path(f.name)).is_ok_and(|m| m.len() == f.size))
    }

    /// Its files on disk, partial downloads included.
    fn files_on_disk(self) -> Vec<(PathBuf, u64)> {
        files_in(&self.dir(), self.files())
    }

    /// Removes its partial downloads that nothing writes any more (left by
    /// a run of the program that stopped while downloading).
    pub fn remove_orphans(self) {
        for f in self.files() {
            remove_orphans(&self.path(f.name));
        }
    }

    /// Bytes of its files present on disk.
    pub fn size_on_disk(self) -> u64 {
        self.files_on_disk().iter().map(|(_, len)| len).sum()
    }

    /// Removes its files from the computer, after freeing its sessions
    /// (Windows refuses to delete a file in use). All are tried; the first
    /// error is returned.
    pub fn delete(self) -> Result<(), String> {
        self.unload();
        let mut result = Ok(());
        for (path, _) in self.files_on_disk() {
            if let Err(e) = std::fs::remove_file(path)
                && result.is_ok()
            {
                result = Err(e.to_string());
            }
        }
        result
    }

    /// Frees its sessions (the memory and the files they hold).
    pub fn unload(self) {
        match self {
            Model::Subject => super::subject::SESSION.unload(),
            Model::Objects => {
                super::objects::ENCODER.unload();
                super::objects::DECODER.unload();
            }
            Model::Inpaint => super::inpaint::SESSION.unload(),
        }
    }

    /// Where its sessions ran in this session of the program (each one
    /// once), if they have run.
    pub fn devices(self) -> Vec<Device> {
        let mut devices = Vec::new();
        let ran = match self {
            Model::Subject => vec![super::subject::SESSION.device()],
            Model::Objects => super::objects::devices().to_vec(),
            Model::Inpaint => vec![super::inpaint::SESSION.device()],
        };
        for device in ran.into_iter().flatten() {
            if !devices.contains(&device) {
                devices.push(device);
            }
        }
        devices
    }

    /// Downloads the missing files, each checked against its digest.
    pub fn download(self, progress: &Progress) -> Result<(), DownloadError> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(failed)?;
        progress.total.store(self.size(), Ordering::Relaxed);
        progress.downloading.store(true, Ordering::Relaxed);
        let done = self.files().iter().try_fold(0, |before, f| {
            let path = self.path(f.name);
            if !std::fs::metadata(&path).is_ok_and(|m| m.len() == f.size) {
                download_file(f, &path, progress, before, WINDOW)?;
            }
            Ok(before + f.size)
        });
        progress.downloading.store(false, Ordering::Relaxed);
        done.map(|_| ())
    }
}

/// `files` present in `dir`, and their partial downloads, with their size.
fn files_in(dir: &Path, files: &[ModelFile]) -> Vec<(PathBuf, u64)> {
    // Partial downloads, and the marks of the files checked.
    let partial = |name: &str| {
        files.iter().any(|f| {
            (name.starts_with(&format!("{}.", f.name)) && name.ends_with(".part"))
                || name == format!("{}{CHECKED_MARK}", f.name)
        })
    };
    let mut found: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(partial))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.len())))
        .collect();
    found.extend(files.iter().filter_map(|f| {
        let path = dir.join(f.name);
        let len = std::fs::metadata(&path).ok()?.len();
        Some((path, len))
    }));
    found
}

/// Why a download did not end well.
#[derive(Debug, PartialEq)]
pub enum DownloadError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => f.write_str(&t!("ai-error-download-cancelled")),
            DownloadError::Failed(e) => f.write_str(e),
        }
    }
}

/// A file of the model could not be written: why, in words.
fn failed(e: std::io::Error) -> DownloadError {
    use std::io::ErrorKind;
    DownloadError::Failed(match e.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => t!("fx3-ai-error-disk-full"),
        ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => {
            t!(
                "fx3-ai-error-no-write",
                folder = root().display().to_string()
            )
        }
        _ => t!("fx3-ai-error-write", error = e.to_string()),
    })
}

/// Longest wait for a connection and for the server's answer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// A request reads the file for at most this long; the download then goes
/// on with a new request from where it stopped. So a connection that stalls
/// is noticed (ureq has no timeout between two reads), and a cancelled
/// download stops within this time.
const WINDOW: Duration = Duration::from_secs(20);

/// File being downloaded, removed unless it was completed: a name of its
/// own, so that a download cancelled but not yet stopped never mixes with
/// the next one.
struct Partial(PathBuf);

impl Partial {
    fn new(path: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        Self(path.with_file_name(format!("{name}.{}-{n}.part", std::process::id())))
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A partial download untouched for this long, of another run of the
/// program, is an orphan.
const ORPHAN_AGE: Duration = Duration::from_secs(120);

/// Removes the partial downloads of `path` that another run of the program
/// left (each run names its own, see [`Partial`]) and stopped writing.
fn remove_orphans(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let (prefix, own) = (
        format!("{name}."),
        format!("{name}.{}-", std::process::id()),
    );
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        if !file.starts_with(&prefix) || !file.ends_with(".part") || file.starts_with(&own) {
            continue;
        }
        // A real stat: on Windows, the directory entry's time only changes
        // once the writer closes the file, so a download in progress of
        // another run would look untouched.
        let age = std::fs::metadata(entry.path())
            .and_then(|m| m.modified())
            .ok();
        if age
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= ORPHAN_AGE)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A new partial download. On Windows, others can't delete it while it is
/// written (another run of the program cleaning its orphans).
fn create_partial(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE, not FILE_SHARE_DELETE.
        options.share_mode(0x1 | 0x2);
    }
    options.open(path)
}

/// A model file that could not be loaded: if it isn't what was downloaded
/// (damaged on the disk), it is removed so that the model is offered for
/// download again. Returns `true` then.
/// Its other files (external data) are checked too.
pub fn remove_if_damaged(path: &Path) -> bool {
    let Some(model) = model_of(path) else {
        return false;
    };
    let mut removed = false;
    for f in model.files() {
        let path = model.path(f.name);
        if digest_of(&path).is_some_and(|d| d != f.sha256) {
            removed |= remove_damaged(&path, f.name);
        }
    }
    removed
}

/// Before the first use of a model in this run of the program (`path`:
/// one of its files): its files that aren't what was downloaded are
/// removed, so that it is offered for download again. Loading a model
/// only notices a damaged graph; damaged weights would give wrong results
/// for ever. Each file is hashed once, then remembered beside it (its size
/// and date). Returns `true` if one was removed. It takes seconds the
/// first time: never on the UI thread.
pub fn remove_damaged_once(path: &Path) -> bool {
    let Some(model) = model_of(path) else {
        return false;
    };
    // One check at a time (two sessions of a model load together).
    let mut checked = checked();
    let mut removed = false;
    for f in model.files() {
        removed |= remove_if_not(&model.path(f.name), f, &mut checked);
    }
    removed
}

/// File `f` at `path` removed if it isn't what was downloaded, unless found
/// right before (in `checked`, or by its mark). Returns `true` if removed.
fn remove_if_not(path: &Path, f: &ModelFile, checked: &mut Vec<PathBuf>) -> bool {
    if checked.iter().any(|p| p == path) {
        return false;
    }
    // Missing: loading says so.
    let Some(now) = stamp(path) else {
        return false;
    };
    if std::fs::read_to_string(mark_of(path)).is_ok_and(|s| s == now) {
        checked.push(path.to_path_buf());
        return false;
    }
    match digest_of(path) {
        Some(d) if d != f.sha256 => remove_damaged(path, f.name),
        Some(_) => {
            mark_checked(path, checked);
            false
        }
        None => false,
    }
}

/// The model `path` is a file of.
fn model_of(path: &Path) -> Option<Model> {
    Model::ALL
        .into_iter()
        .find(|m| m.files().iter().any(|f| m.path(f.name) == path))
}

fn remove_damaged(path: &Path, name: &str) -> bool {
    crate::diag::warn(crate::i18n::tm!("ui4-log-model-damaged", name = name));
    let _ = std::fs::remove_file(mark_of(path));
    std::fs::remove_file(path).is_ok()
}

/// Model files found right in this run of the program.
fn checked() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    static CHECKED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    CHECKED.lock().unwrap_or_else(|e| e.into_inner())
}

/// What tells, beside a model file, that it was found right: its
/// [`stamp`] then.
fn mark_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(CHECKED_MARK);
    PathBuf::from(name)
}

const CHECKED_MARK: &str = ".checked";

/// Size and modification time of a file.
fn stamp(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(format!("{} {}", m.len(), t.as_nanos()))
}

fn mark_checked(path: &Path, checked: &mut Vec<PathBuf>) {
    if let Some(now) = stamp(path) {
        let _ = std::fs::write(mark_of(path), now);
    }
    if !checked.iter().any(|p| p == path) {
        checked.push(path.to_path_buf());
    }
}

/// The SHA-256 of a file, in hexadecimal.
fn digest_of(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Network error, in words: a timeout says that the connection stopped
/// responding.
fn network(e: impl Into<ureq::Error>) -> DownloadError {
    use ureq::Error;
    DownloadError::Failed(match e.into() {
        Error::Timeout(_) | Error::BodyStalled => t!("ai-error-stalled"),
        Error::HostNotFound | Error::ConnectionFailed | Error::Io(_) => {
            t!("fx3-ai-error-offline")
        }
        Error::StatusCode(code) => t!("fx3-ai-error-status", code = code.to_string()),
        Error::Tls(_) => t!("fx3-ai-error-secure"),
        e => e.to_string(),
    })
}

/// `file` to `path`, through a `.part` file renamed once checked.
/// `before`: bytes of the model already counted in `progress`.
fn download_file(
    file: &ModelFile,
    path: &Path,
    progress: &Progress,
    before: u64,
    window: Duration,
) -> Result<(), DownloadError> {
    let agent = |window: Option<Duration>| -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(RESPONSE_TIMEOUT))
            .timeout_recv_body(window)
            .build()
            .into()
    };
    let (windowed, whole) = (agent(Some(window)), agent(None));
    remove_orphans(path);
    let partial = Partial::new(path);
    let mut out = create_partial(&partial.0).map_err(failed)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut total = 0u64;
    // Does the server send the rest of a file (HTTP range)? Otherwise, the
    // file is read in one request, without time limit.
    let mut ranges = true;
    while total < file.size {
        let start = total;
        let request = if ranges { &windowed } else { &whole }.get(file.url);
        let request = if start > 0 {
            request.header("Range", format!("bytes={start}-"))
        } else {
            request
        };
        let response = request.call().map_err(network)?;
        let resumed = response.status() == 206
            && response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with(&format!("bytes {start}-")));
        if start > 0 && !resumed {
            // From the start again, in one request.
            ranges = false;
            total = 0;
            hasher = Sha256::new();
            out.set_len(0).map_err(failed)?;
            out.rewind().map_err(failed)?;
        }
        let start = total;
        let mut reader = response.into_body().into_reader();
        let mut lost = None;
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(DownloadError::Cancelled);
            }
            let n = match reader.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    lost = Some(e);
                    break;
                }
            };
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > file.size {
                return Err(DownloadError::Failed(t!("ai-error-model-corrupt")));
            }
            hasher.update(&buffer[..n]);
            out.write_all(&buffer[..n]).map_err(failed)?;
            progress.downloaded.store(before + total, Ordering::Relaxed);
        }
        // End of the request's time, connection lost or closed early: goes
        // on with another request if this one brought something.
        if total == start || (lost.is_some() && !ranges) {
            let interrupted = || DownloadError::Failed(t!("ai-error-download-interrupted"));
            return Err(lost.map_or_else(interrupted, network));
        }
    }
    drop(out);
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if digest != file.sha256 {
        return Err(DownloadError::Failed(t!("ai-error-model-corrupt")));
    }
    super::rename(&partial.0, path).map_err(failed)?;
    // Just checked: not hashed again before its first use.
    mark_checked(path, &mut checked());
    Ok(())
}

/// Models folder: `%LOCALAPPDATA%\Atelier\modeles` on Windows,
/// `~/.cache/atelier` elsewhere.
pub fn root() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("Atelier").join("modeles");
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(cache).join("atelier");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache").join("atelier")
}

/// Download progress, shared with the UI.
#[derive(Default)]
pub struct Progress {
    pub downloaded: AtomicU64,
    pub total: AtomicU64,
    /// True while downloading (false while computing, for jobs that do both).
    pub downloading: AtomicBool,
    /// Set by the UI to stop the download.
    pub cancel: AtomicBool,
}

impl Progress {
    /// Downloaded fraction (0..1).
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed).max(1);
        self.downloaded.load(Ordering::Relaxed) as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::Instant;

    /// What the test server does with each request for `body`: sends
    /// `cut` bytes at most (from the requested start) and nothing past
    /// `end`, then stalls or closes; with `ranges`, it honours HTTP ranges.
    #[derive(Clone, Copy)]
    struct Server {
        cut: usize,
        end: usize,
        stall: bool,
        ranges: bool,
    }

    fn serve(body: Vec<u8>, server: Server) -> &'static str {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let body = body.clone();
                std::thread::spawn(move || answer(stream.unwrap(), &body, server));
            }
        });
        Box::leak(format!("http://127.0.0.1:{port}/f").into_boxed_str())
    }

    fn answer(mut s: TcpStream, body: &[u8], server: Server) {
        let mut request = Vec::new();
        let mut byte = [0u8];
        while !request.ends_with(b"\r\n\r\n") && s.read(&mut byte).is_ok_and(|n| n == 1) {
            request.push(byte[0]);
        }
        let request = String::from_utf8_lossy(&request).to_lowercase();
        let start = request
            .lines()
            .find_map(|l| l.strip_prefix("range: bytes="))
            .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
            .filter(|_| server.ranges)
            .unwrap_or(0);
        let rest = &body[start..];
        let head = if start > 0 {
            let range = format!("bytes {start}-{}/{}", body.len() - 1, body.len());
            format!("HTTP/1.1 206 Partial Content\r\nContent-Range: {range}\r\n")
        } else {
            "HTTP/1.1 200 OK\r\n".to_owned()
        };
        let _ = write!(s, "{head}Content-Length: {}\r\n\r\n", rest.len());
        let sent = server.cut.min(server.end.saturating_sub(start));
        let _ = s.write_all(&rest[..rest.len().min(sent)]);
        let _ = s.flush();
        if server.stall {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    fn body(len: usize) -> (Vec<u8>, &'static str) {
        let body: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
        let digest: String = Sha256::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        (body, Box::leak(digest.into_boxed_str()))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(url: &'static str, size: usize, sha256: &'static str) -> ModelFile {
        ModelFile {
            name: "model.onnx",
            url,
            size: size as u64,
            sha256,
        }
    }

    fn parts(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "part")
            })
            .count()
    }

    /// A connection lost midway: the download goes on from where it
    /// stopped, and the file is complete.
    #[test]
    fn download_resumes_after_a_lost_connection() {
        let (data, digest) = body(300_000);
        let server = Server {
            cut: 70_000,
            end: usize::MAX,
            stall: false,
            ranges: true,
        };
        let url = serve(data.clone(), server);
        let dir = temp_dir("resume");
        let path = dir.join("model.onnx");
        let progress = Progress::default();
        let f = file(url, data.len(), digest);
        download_file(&f, &path, &progress, 0, WINDOW).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(parts(&dir), 0);
    }

    /// A server that ignores ranges: the file is read again from the
    /// start, in one request; if it still stops, the download fails
    /// without leaving its partial file.
    #[test]
    fn failed_download_leaves_no_partial_file() {
        let (data, digest) = body(300_000);
        let server = Server {
            cut: 100_000,
            end: usize::MAX,
            stall: false,
            ranges: false,
        };
        let url = serve(data.clone(), server);
        let dir = temp_dir("cut");
        let path = dir.join("model.onnx");
        let f = file(url, data.len(), digest);
        let result = download_file(&f, &path, &Progress::default(), 0, WINDOW);
        assert!(
            matches!(result, Err(DownloadError::Failed(_))),
            "{result:?}"
        );
        assert!(!path.exists());
        assert_eq!(parts(&dir), 0);
    }

    /// A connection that stalls (no timeout between reads in ureq): the
    /// download fails after its time window, and stops then if cancelled.
    #[test]
    fn stalled_download_fails_and_cancel_stops_it() {
        let (data, digest) = body(300_000);
        let server = Server {
            cut: usize::MAX,
            end: 1000,
            stall: true,
            ranges: true,
        };
        let url = serve(data.clone(), server);
        let dir = temp_dir("stall");
        let f = file(url, data.len(), digest);
        let window = Duration::from_millis(500);
        let t = Instant::now();
        let result = download_file(&f, &dir.join("model.onnx"), &Progress::default(), 0, window);
        let stalled = DownloadError::Failed(t!("ai-error-stalled"));
        assert_eq!(result, Err(stalled));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());

        let progress = Arc::new(Progress::default());
        let shared = Arc::clone(&progress);
        let path = dir.join("model.onnx");
        let (send, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let f = file(url, data.len(), digest);
            let _ = send.send(download_file(&f, &path, &shared, 0, window));
        });
        std::thread::sleep(Duration::from_millis(200));
        progress.cancel.store(true, Ordering::Relaxed);
        let result = done.recv_timeout(Duration::from_secs(5));
        assert_eq!(result, Ok(Err(DownloadError::Cancelled)));
        assert_eq!(parts(&dir), 0);
    }

    /// A model file of the right size whose bytes changed (damaged weights,
    /// which load without error) is found by its hash and removed; one
    /// found right is not hashed again while it doesn't change.
    #[test]
    fn damaged_model_files_are_found_by_their_hash_once() {
        const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let dir = temp_dir("checked");
        let path = dir.join("model.onnx");
        std::fs::write(&path, b"abc").unwrap();
        let mut checked = Vec::new();
        assert!(!remove_if_not(&path, &file("", 3, ABC), &mut checked));
        assert!(mark_of(&path).exists());
        // Found right in an earlier run (its mark): not hashed again, even
        // if the expected digest were another.
        let mut later = Vec::new();
        assert!(!remove_if_not(&path, &file("", 3, "0"), &mut later));
        assert!(path.exists());
        // Changed since: hashed again, removed with its mark.
        std::fs::write(&path, b"abcd").unwrap();
        let mut again = Vec::new();
        assert!(remove_if_not(&path, &file("", 4, ABC), &mut again));
        assert!(!path.exists() && !mark_of(&path).exists());
    }

    /// Partial files (from a download cut short by a crash) count on disk,
    /// and are deleted with the model.
    #[test]
    fn partial_files_count_on_disk() {
        let dir = temp_dir("disk");
        std::fs::write(dir.join("model.onnx.123-0.part"), [0u8; 40]).unwrap();
        std::fs::write(dir.join("model.onnx"), [0u8; 2]).unwrap();
        std::fs::write(dir.join("other.onnx.part"), [0u8; 5]).unwrap();
        let files = [file("", 2, "")];
        let found = files_in(&dir, &files);
        assert_eq!(found.iter().map(|(_, len)| len).sum::<u64>(), 42);
    }

    #[test]
    fn digest_of_a_file() {
        let dir = temp_dir("digest");
        let path = dir.join("model.onnx");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            digest_of(&path).as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        // Not a model file: never removed.
        assert!(!remove_if_damaged(&path));
        assert!(path.exists());
    }

    #[test]
    fn orphan_partial_files_are_removed() {
        let dir = temp_dir("orphans");
        let part = |name: &str, age: u64| {
            let path = dir.join(name);
            let f = std::fs::File::create(&path).unwrap();
            f.set_modified(std::time::SystemTime::now() - Duration::from_secs(age))
                .unwrap();
            path
        };
        let own = format!("model.onnx.{}-0.part", std::process::id());
        let old = part("model.onnx.1-0.part", 600);
        let recent = part("model.onnx.2-0.part", 5);
        let mine = part(&own, 600);
        let other = part("other.onnx.1-0.part", 600);
        remove_orphans(&dir.join("model.onnx"));
        assert!(!old.exists());
        assert!(recent.exists() && mine.exists() && other.exists());
    }
}
