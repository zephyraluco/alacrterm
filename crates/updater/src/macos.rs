//! All functions must be called from the AppKit/GPUI main thread.
use anyhow::{Result, bail};

#[cfg(sparkle)]
unsafe extern "C" {
    fn alacrterm_sparkle_start(automatic: bool) -> bool;
    fn alacrterm_sparkle_check();
    fn alacrterm_sparkle_automatic(enabled: bool);
    fn alacrterm_sparkle_status() -> *const std::ffi::c_char;
    fn alacrterm_sparkle_state() -> i32;
    fn alacrterm_sparkle_changed() -> bool;
    fn alacrterm_sparkle_restart_requested() -> bool;
    fn alacrterm_sparkle_allow_restart();
}

pub fn start(automatic: bool) -> Result<()> {
    #[cfg(sparkle)]
    if unsafe { alacrterm_sparkle_start(automatic) } {
        return Ok(());
    }
    #[cfg(not(sparkle))]
    let _ = automatic;
    bail!("{}", status())
}
pub fn check() {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_check();
    }
}
pub fn set_automatic(enabled: bool) {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_automatic(enabled);
    }
    #[cfg(not(sparkle))]
    let _ = enabled;
}
pub fn status() -> String {
    #[cfg(sparkle)]
    unsafe {
        let text = alacrterm_sparkle_status();
        if !text.is_null() {
            return std::ffi::CStr::from_ptr(text)
                .to_string_lossy()
                .into_owned();
        }
    }
    "此构建未集成 Sparkle，请从发布页下载安装".into()
}
pub fn state() -> i32 {
    #[cfg(sparkle)]
    unsafe {
        return alacrterm_sparkle_state();
    }
    #[cfg(not(sparkle))]
    0
}
pub fn changed() -> bool {
    #[cfg(sparkle)]
    unsafe {
        return alacrterm_sparkle_changed();
    }
    #[cfg(not(sparkle))]
    false
}
pub fn take_restart_request() -> bool {
    #[cfg(sparkle)]
    unsafe {
        return alacrterm_sparkle_restart_requested();
    }
    #[cfg(not(sparkle))]
    false
}
pub fn allow_restart() {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_allow_restart();
    }
}
