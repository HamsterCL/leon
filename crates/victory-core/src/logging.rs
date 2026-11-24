//! One tracing setup for every binary.
//!
//! Filter with `VICTORY_LOG` (falls back to `RUST_LOG`), e.g.
//! `VICTORY_LOG=victory_loader=trace,info`. Debug and trace output exist
//! only in builds with the `diagnostics` feature; a plain release build
//! logs info and above whatever the filter asks for.
//!
//! `VICTORY_LOGS=1` shows the log, in a console of its own when the process
//! has none; `VICTORY_LOGS=0` turns it off, and hides the console when this
//! process is the only one using it. Unset, the log goes to the console the
//! program was started from, as it always has.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The most verbose level this build can log.
const CAP: LevelFilter = if cfg!(feature = "diagnostics") {
    LevelFilter::TRACE
} else {
    LevelFilter::INFO
};

/// What `VICTORY_LOGS` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logs {
    /// Not set, or not a `1` or a `0`: the log as it always was.
    Default,
    On,
    Off,
}

/// Reads the value of `VICTORY_LOGS`.
pub fn logs_setting(value: Option<&str>) -> Logs {
    match value.map(str::trim) {
        Some("1") => Logs::On,
        Some("0") => Logs::Off,
        _ => Logs::Default,
    }
}

/// Gives the log its console, or takes it away.
#[cfg(windows)]
fn console(setting: Logs) {
    use windows_sys::Win32::System::Console::{
        AllocConsole, GetConsoleProcessList, GetConsoleWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    // SAFETY: plain console queries and calls; a null window is checked.
    unsafe {
        let window = GetConsoleWindow();
        match setting {
            Logs::On if window.is_null() => {
                AllocConsole();
            }
            Logs::Off if !window.is_null() => {
                // Only ours to hide when no other process shares it: a
                // terminal the program was started from stays.
                let mut sharing = [0_u32; 2];
                if GetConsoleProcessList(sharing.as_mut_ptr(), 2) == 1 {
                    ShowWindow(window, SW_HIDE);
                }
            }
            _ => {}
        }
    }
}

#[cfg(not(windows))]
fn console(_: Logs) {}

pub fn init(default_level: &str) {
    let setting = logs_setting(std::env::var("VICTORY_LOGS").ok().as_deref());
    console(setting);
    let filter = std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level));
    let _ = tracing_subscriber::registry()
        .with(if setting == Logs::Off { LevelFilter::OFF } else { CAP })
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_writer(std::io::stderr),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_and_zero_are_the_only_switches() {
        assert_eq!(logs_setting(Some("1")), Logs::On);
        assert_eq!(logs_setting(Some("0")), Logs::Off);
        assert_eq!(logs_setting(Some(" 1 ")), Logs::On);
        for other in [None, Some(""), Some("2"), Some("on"), Some("true"), Some("10")] {
            assert_eq!(logs_setting(other), Logs::Default, "{other:?}");
        }
    }
}
