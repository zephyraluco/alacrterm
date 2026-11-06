//! All functions must be called from the AppKit/GPUI main thread.
use anyhow::{Result, bail};

#[cfg(sparkle)]
unsafe extern "C" {
    fn alacrterm_sparkle_start(automatic: bool) -> bool;
    fn alacrterm_sparkle_check();
    fn alacrterm_sparkle_automatic(enabled: bool);
    fn alacrterm_sparkle_version() -> *const std::ffi::c_char;
    fn alacrterm_sparkle_status() -> *const std::ffi::c_char;
    fn alacrterm_sparkle_state() -> i32;
    fn alacrterm_sparkle_changed() -> bool;
    fn alacrterm_sparkle_restart_requested() -> bool;
    fn alacrterm_sparkle_allow_restart() -> bool;
    fn alacrterm_sparkle_notes() -> *const std::ffi::c_char;
    fn alacrterm_sparkle_progress() -> f64;
    fn alacrterm_sparkle_install();
    fn alacrterm_sparkle_defer();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Checking,
    Available,
    Downloading,
    WaitingRestart,
    Failed,
    Deferred,
    Current,
    Installing,
}
impl State {
    pub fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Checking,
            2 => Self::Available,
            3 => Self::Downloading,
            4 => Self::WaitingRestart,
            5 => Self::Failed,
            6 => Self::Deferred,
            7 => Self::Current,
            8 => Self::Installing,
            _ => Self::Idle,
        }
    }
    pub fn busy(self) -> bool {
        matches!(self, Self::Checking | Self::Downloading | Self::Installing)
    }
    pub fn has_update(self) -> bool {
        matches!(
            self,
            Self::Available
                | Self::Downloading
                | Self::WaitingRestart
                | Self::Deferred
                | Self::Installing
        )
    }
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
pub fn available_version() -> Option<String> {
    #[cfg(sparkle)]
    unsafe {
        let text = alacrterm_sparkle_version();
        if !text.is_null() {
            let value = std::ffi::CStr::from_ptr(text).to_string_lossy();
            if !value.is_empty() {
                return Some(value.into_owned());
            }
        }
    }
    None
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
        alacrterm_sparkle_state()
    }
    #[cfg(not(sparkle))]
    0
}
pub fn changed() -> bool {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_changed()
    }
    #[cfg(not(sparkle))]
    false
}
pub fn take_restart_request() -> bool {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_restart_requested()
    }
    #[cfg(not(sparkle))]
    false
}
pub fn allow_restart() -> bool {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_allow_restart()
    }
    #[cfg(not(sparkle))]
    false
}

pub fn notes() -> String {
    #[cfg(sparkle)]
    unsafe {
        let text = alacrterm_sparkle_notes();
        if !text.is_null() {
            return std::ffi::CStr::from_ptr(text)
                .to_string_lossy()
                .into_owned();
        }
    }
    String::new()
}

pub fn progress() -> Option<f32> {
    #[cfg(sparkle)]
    {
        let value = unsafe { alacrterm_sparkle_progress() };
        (value.is_finite() && value >= 0.0).then_some(value.clamp(0.0, 100.0) as f32)
    }
    #[cfg(not(sparkle))]
    None
}

pub fn install() {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_install();
    }
}

pub fn defer() {
    #[cfg(sparkle)]
    unsafe {
        alacrterm_sparkle_defer();
    }
}

#[cfg(test)]
mod tests {
    use super::State;
    #[test]
    fn native_states_distinguish_active_work_from_deferred_and_completed_cycles() {
        assert!(State::from_code(1).busy());
        assert!(State::from_code(3).busy());
        assert!(State::from_code(3).has_update());
        for code in [2, 4, 6] {
            assert!(State::from_code(code).has_update());
            assert!(!State::from_code(code).busy());
        }
        assert_eq!(State::from_code(7), State::Current);
        assert!(State::from_code(8).busy());
        assert!(State::from_code(8).has_update());
        for code in [0, 5, 7, 99] {
            assert!(!State::from_code(code).has_update());
        }
    }
}
