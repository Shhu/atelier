//! The monitor's profile, as the system gives it: Windows' display profile
//! of the monitor showing the window (GetICMProfile), the `_ICC_PROFILE` the
//! desktop publishes on X11. macOS converts the window from sRGB to the
//! display itself: nothing to do. Followed while Atelier runs: the window
//! moved to another monitor, or the profile changed in the system settings.

use std::sync::Arc;
use std::time::Duration;

use super::{Profile, Space};

/// The main monitor's RGB profile, if the system has one.
pub fn monitor_profile() -> Option<Profile> {
    if cfg!(test) {
        return None;
    }
    profile_of(system_profile(None)?)
}

/// `icc` if it is an RGB profile.
pub fn profile_of(icc: Vec<u8>) -> Option<Profile> {
    Profile::from_icc(Arc::new(icc)).filter(|p| p.space() == Space::Rgb)
}

/// How often the profile is looked at again.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// Looks at the monitor's profile every few seconds on a thread of its own
/// (the monitor showing `window` on Windows, its HWND), and calls `changed`
/// with the new one (`None`: none any more) when it differs from the main
/// monitor's at start. Ends when `changed` returns false.
pub fn watch(window: Option<isize>, changed: impl Fn(Option<Vec<u8>>) -> bool + Send + 'static) {
    if cfg!(test) || cfg!(not(any(windows, target_os = "linux"))) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("monitor profile".into())
        .spawn(move || {
            let mut last = system_profile(None);
            loop {
                std::thread::sleep(WATCH_EVERY);
                let now = system_profile(window);
                if now != last {
                    last.clone_from(&now);
                    if !changed(now) {
                        return;
                    }
                }
            }
        });
}

#[cfg(windows)]
fn system_profile(window: Option<isize>) -> Option<Vec<u8>> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;

    #[repr(C)]
    struct MonitorInfo {
        size: u32,
        monitor: [i32; 4],
        work: [i32; 4],
        flags: u32,
        device: [u16; 32],
    }
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(window: *mut c_void) -> *mut c_void;
        fn ReleaseDC(window: *mut c_void, dc: *mut c_void) -> i32;
        fn MonitorFromWindow(window: isize, flags: u32) -> *mut c_void;
        fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfo) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetICMProfileW(dc: *mut c_void, size: *mut u32, name: *mut u16) -> i32;
        fn CreateDCW(
            driver: *const u16,
            device: *const u16,
            port: *const u16,
            mode: *const c_void,
        ) -> *mut c_void;
        fn DeleteDC(dc: *mut c_void) -> i32;
    }
    // SAFETY: a device context of the window's monitor (or of the whole
    // screen), released below; the buffers hold the sizes given.
    let path = unsafe {
        let mut info = MonitorInfo {
            size: size_of::<MonitorInfo>() as u32,
            monitor: [0; 4],
            work: [0; 4],
            flags: 0,
            device: [0; 32],
        };
        let monitor = window.map(|w| MonitorFromWindow(w, MONITOR_DEFAULTTONEAREST));
        let own = monitor.is_some_and(|m| !m.is_null() && GetMonitorInfoW(m, &mut info) != 0);
        let dc = match own {
            true => CreateDCW(
                info.device.as_ptr(),
                info.device.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            ),
            false => GetDC(std::ptr::null_mut()),
        };
        if dc.is_null() {
            return None;
        }
        let mut name = vec![0u16; 1024];
        let mut size = name.len() as u32;
        let ok = GetICMProfileW(dc, &mut size, name.as_mut_ptr());
        match own {
            true => DeleteDC(dc),
            false => ReleaseDC(std::ptr::null_mut(), dc),
        };
        if ok == 0 {
            return None;
        }
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        std::ffi::OsString::from_wide(&name[..len])
    };
    std::fs::read(path).ok()
}

#[cfg(target_os = "linux")]
fn system_profile(_window: Option<isize>) -> Option<Vec<u8>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = conn
        .intern_atom(true, b"_ICC_PROFILE")
        .ok()?
        .reply()
        .ok()?
        .atom;
    if atom == 0 {
        return None;
    }
    let reply = conn
        .get_property(false, root, atom, AtomEnum::ANY, 0, u32::MAX / 4)
        .ok()?
        .reply()
        .ok()?;
    (!reply.value.is_empty()).then_some(reply.value)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn system_profile(_window: Option<isize>) -> Option<Vec<u8>> {
    None
}
