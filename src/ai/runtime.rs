//! ONNX Runtime: on Windows, the copy embedded in the .exe (with DirectML,
//! for the graphics card); elsewhere, the one linked in. The models'
//! sessions: loaded on first use, freed when unused.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::value::{PrimitiveTensorElementType, TensorRef};

/// Where a model runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    /// Graphics card, through DirectML (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Gpu,
    Cpu,
}

/// Unused for this long, a session leaves memory (hundreds of MB).
const IDLE: Duration = Duration::from_secs(90);

/// Loads ONNX Runtime: before any other call to it. On Windows, `ort`
/// would otherwise load the system's onnxruntime.dll by itself (too old,
/// or absent) and panic, leaving it unusable for the whole session.
fn init() -> Result<(), String> {
    #[cfg(windows)]
    return windows::init();
    #[cfg(not(windows))]
    Ok(())
}

/// Proof that ONNX Runtime is loaded, given to the code that runs a model:
/// its tensors can only be made with it.
#[derive(Clone, Copy)]
pub struct Loaded(());

impl Loaded {
    /// Tensor of `shape` over `data` (not copied).
    pub fn tensor<'a, T>(self, shape: &[usize], data: &'a [T]) -> ort::Result<TensorRef<'a, T>>
    where
        T: PrimitiveTensorElementType + std::fmt::Debug + Clone + 'static,
    {
        TensorRef::from_array_view((shape.to_vec(), data))
    }
}

/// Session of a model file: loaded on first use, freed once unused for
/// `IDLE`, or before the model is deleted. On Windows, on the graphics
/// card when it accepts the model, otherwise (and from its first failure
/// on) on the CPU.
pub struct Slot {
    path: fn() -> PathBuf,
    state: Mutex<SlotState>,
    /// Where it last ran (0: not yet), readable while it runs.
    device: AtomicU8,
}

struct SlotState {
    session: Option<(Session, Device)>,
    used: Option<Instant>,
    /// The graphics card failed: CPU only.
    cpu_only: bool,
    /// A thread frees the session once unused.
    reaping: bool,
}

impl Slot {
    pub const fn new(path: fn() -> PathBuf) -> Self {
        Self {
            path,
            state: Mutex::new(SlotState {
                session: None,
                used: None,
                cpu_only: false,
                reaping: false,
            }),
            device: AtomicU8::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|e: PoisonError<_>| {
            // A panic during a run: the session is in an unknown state.
            let mut state = e.into_inner();
            state.session = None;
            self.state.clear_poison();
            state
        })
    }

    /// Runs `f` on the session. If the graphics card fails, the model moves
    /// to the CPU and `f` runs again there.
    pub fn run<R>(
        &'static self,
        mut f: impl FnMut(&mut Session, Loaded) -> ort::Result<R>,
    ) -> Result<R, String> {
        init()?;
        let mut state = self.lock();
        let result = self.run_locked(&mut state, &mut f);
        state.used = Some(Instant::now());
        if state.session.is_some() && !state.reaping {
            state.reaping = true;
            self.free_when_unused();
        }
        result
    }

    fn run_locked<R>(
        &self,
        state: &mut SlotState,
        f: &mut impl FnMut(&mut Session, Loaded) -> ort::Result<R>,
    ) -> Result<R, String> {
        let (session, device) = match &mut state.session {
            Some(loaded) => loaded,
            empty => empty.insert(self.load(state.cpu_only)?),
        };
        let device = *device;
        match f(session, Loaded(())) {
            Err(e) if device == Device::Gpu => {
                let model = (self.path)();
                let name = model.file_name().unwrap_or_default().to_string_lossy();
                crate::diag::warn(crate::i18n::tm!(
                    "ui4-log-ai-gpu-failed",
                    name = name.to_string(),
                    error = crate::i18n::Message::of(|| e.to_string())
                ));
                state.cpu_only = true;
                state.session = None;
                let (session, _) = state.session.insert(self.load(true)?);
                f(session, Loaded(())).map_err(|e| e.to_string())
            }
            result => result.map_err(|e| e.to_string()),
        }
    }

    fn load(&self, cpu_only: bool) -> Result<(Session, Device), String> {
        let path = (self.path)();
        // Damaged weights load fine: found by their hash, once.
        if super::models::remove_damaged_once(&path) {
            return Err(crate::i18n::t!("fx4-ai-model-damaged"));
        }
        let loaded = session(&path, cpu_only).map_err(|e| {
            // Of the right size but damaged: removed, to download again.
            if super::models::remove_if_damaged(&path) {
                crate::i18n::t!("fx4-ai-model-damaged")
            } else {
                e
            }
        })?;
        let code = match loaded.1 {
            Device::Gpu => 1,
            Device::Cpu => 2,
        };
        self.device.store(code, Ordering::Relaxed);
        Ok(loaded)
    }

    fn free_when_unused(&'static self) {
        std::thread::spawn(move || {
            loop {
                let wait = {
                    let mut state = self.lock();
                    let idle = state.used.map_or(IDLE, |t| t.elapsed());
                    if state.session.is_none() || idle >= IDLE {
                        state.session = None;
                        state.reaping = false;
                        return;
                    }
                    IDLE - idle
                };
                std::thread::sleep(wait);
            }
        });
    }

    /// Frees the session now (it keeps the model's files open). Waits for
    /// a run in progress.
    pub fn unload(&self) {
        self.lock().session = None;
    }

    /// Where it ran last, if it has run in this session.
    pub fn device(&self) -> Option<Device> {
        match self.device.load(Ordering::Relaxed) {
            1 => Some(Device::Gpu),
            2 => Some(Device::Cpu),
            _ => None,
        }
    }
}

/// Loads a model: on Windows on the graphics card (DirectML) when it
/// accepts it and `cpu_only` is false, otherwise on the CPU.
fn session(model: &Path, cpu_only: bool) -> Result<(Session, Device), String> {
    #[cfg(windows)]
    if !cpu_only {
        match gpu_session(model) {
            Ok(session) => return Ok((session, Device::Gpu)),
            Err(e) => crate::diag::warn(crate::i18n::tm!(
                "ui4-log-ai-gpu-not-loaded",
                name = model
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                error = crate::i18n::Message::of(|| e.to_string())
            )),
        }
    }
    #[cfg(not(windows))]
    let _ = cpu_only;
    // Without the CPU arena, the memory of a run is returned once it ends.
    let cpu = ort::ep::CPU::default().with_arena_allocator(false).build();
    let session = Session::builder()
        .and_then(|b| b.with_execution_providers([cpu]).map_err(Into::into))
        .and_then(|mut b| b.commit_from_file(model))
        .map_err(|e| e.to_string())?;
    Ok((session, Device::Cpu))
}

/// Session on DirectML only: an error rather than a silent fallback, so
/// that the device shown is the real one.
#[cfg(windows)]
fn gpu_session(model: &Path) -> Result<Session, String> {
    // DirectML allows neither memory reuse nor parallel execution.
    Session::builder()
        .and_then(|b| b.with_memory_pattern(false).map_err(Into::into))
        .and_then(|b| b.with_parallel_execution(false).map_err(Into::into))
        .and_then(|b| {
            b.with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()])
                .map_err(Into::into)
        })
        .and_then(|mut b| b.commit_from_file(model))
        .map_err(|e| e.to_string())
}

/// On Windows, ONNX Runtime, DirectML and the Visual C++ DLLs they need are
/// embedded in the .exe: a single file to distribute. On the first use of a
/// model, they are extracted to `%LOCALAPPDATA%\Atelier` and then loaded.
#[cfg(windows)]
mod windows {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    use crate::i18n::t;

    const VERSION: &str = "1.24.4";

    /// In load order: dependencies first.
    const FILES: [(&str, &[u8]); 6] = [
        (
            "vcruntime140.dll",
            include_bytes!("../../packaging/windows/runtime/vcruntime140.dll"),
        ),
        (
            "vcruntime140_1.dll",
            include_bytes!("../../packaging/windows/runtime/vcruntime140_1.dll"),
        ),
        (
            "msvcp140.dll",
            include_bytes!("../../packaging/windows/runtime/msvcp140.dll"),
        ),
        (
            "msvcp140_1.dll",
            include_bytes!("../../packaging/windows/runtime/msvcp140_1.dll"),
        ),
        (
            "DirectML.dll",
            include_bytes!("../../packaging/windows/runtime/DirectML.dll"),
        ),
        (
            "onnxruntime.dll",
            include_bytes!("../../packaging/windows/runtime/onnxruntime.dll"),
        ),
    ];

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
        fn SetDllDirectoryW(path: *const u16) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    /// Extracts and loads ONNX Runtime; before any other call to it. Only a
    /// success is kept: a failure (file held by an antivirus, full disk) is
    /// tried again next time.
    pub fn init() -> Result<(), String> {
        static DONE: Mutex<bool> = Mutex::new(false);
        let mut done = DONE.lock().unwrap_or_else(PoisonError::into_inner);
        if !*done {
            load()?;
            *done = true;
        }
        Ok(())
    }

    fn load() -> Result<(), String> {
        let local =
            std::env::var_os("LOCALAPPDATA").ok_or_else(|| t!("ai-error-no-localappdata"))?;
        let dir = PathBuf::from(local)
            .join("Atelier")
            .join(format!("onnxruntime-{VERSION}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, bytes) in FILES {
            let path = dir.join(name);
            let present = std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
            if !present {
                // Written alongside (a name of its own per process) then
                // renamed: never a half-written DLL.
                let partial = dir.join(format!("{name}.{}.part", std::process::id()));
                let written = std::fs::write(&partial, bytes)
                    .and_then(|()| crate::ai::rename(&partial, &path));
                if let Err(e) = written {
                    let _ = std::fs::remove_file(&partial);
                    return Err(e.to_string());
                }
            }
        }
        // Dependencies are loaded from this directory, not from the system.
        // SAFETY: zero-terminated paths, valid during the call.
        unsafe {
            SetDllDirectoryW(wide(&dir).as_ptr());
            for (name, _) in &FILES[..FILES.len() - 1] {
                if LoadLibraryW(wide(&dir.join(name)).as_ptr()).is_null() {
                    return Err(t!("ai-error-dll-not-loaded", name = *name));
                }
            }
        }
        ort::init_from(dir.join("onnxruntime.dll"))
            .map_err(|e| e.to_string())?
            .commit();
        Ok(())
    }
}

#[cfg(test)]
impl Slot {
    pub fn loaded(&self) -> bool {
        self.lock().session.is_some()
    }
}
