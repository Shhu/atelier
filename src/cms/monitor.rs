//! The monitor's profile, as the system gives it: Windows' display profile
//! (GetICMProfile), the `_ICC_PROFILE` the desktop publishes on X11. macOS
//! converts the window from sRGB to the display itself: nothing to do.

use std::sync::Arc;

use super::{Profile, Space};

/// The main monitor's RGB profile, if the system has one.
pub fn monitor_profile() -> Option<Profile> {
    if cfg!(test) {
        return None;
    }
    let icc = system_profile()?;
    Profile::from_icc(Arc::new(icc)).filter(|p| p.space() == Space::Rgb)
}

#[cfg(windows)]
fn system_profile() -> Option<Vec<u8>> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(window: *mut c_void) -> *mut c_void;
        fn ReleaseDC(window: *mut c_void, dc: *mut c_void) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetICMProfileW(dc: *mut c_void, size: *mut u32, name: *mut u16) -> i32;
    }
    // SAFETY: the screen's device context, released below; the buffer
    // holds `size` characters.
    let path = unsafe {
        let dc = GetDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut name = vec![0u16; 1024];
        let mut size = name.len() as u32;
        let ok = GetICMProfileW(dc, &mut size, name.as_mut_ptr());
        ReleaseDC(std::ptr::null_mut(), dc);
        if ok == 0 {
            return None;
        }
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        std::ffi::OsString::from_wide(&name[..len])
    };
    std::fs::read(path).ok()
}

#[cfg(target_os = "linux")]
fn system_profile() -> Option<Vec<u8>> {
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
fn system_profile() -> Option<Vec<u8>> {
    None
}
