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
//!
//! The log is also kept in a file, one per process, in
//! `%LOCALAPPDATA%\victory\logs` (`VICTORY_LOG_DIR` names another folder):
//! `victory-<unix seconds>-<pid>.log`, the last [`KEEP`] kept. A title that
//! starts itself again writes its own file and leaves the first one alone.
//! `VICTORY_LOGS=0` writes none.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// How many log files stay in the folder.
const KEEP: usize = 10;

/// Where the log files go.
fn log_dir() -> Option<PathBuf> {
    std::env::var_os("VICTORY_LOG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("victory").join("logs"))
        })
}

/// Deletes the oldest `victory-*.log` files until `keep` are left; names
/// sort by their time, so the order of the names is the order of age.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("victory-") && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    let extra = logs.len().saturating_sub(keep);
    for old in &logs[..extra] {
        let _ = std::fs::remove_file(old);
    }
}

/// This process's log file, in a folder kept to [`KEEP`] files.
fn log_file() -> Option<Mutex<File>> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    // Room for the one made now.
    prune(&dir, KEEP - 1);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = dir.join(format!("victory-{secs:012}-{}.log", std::process::id()));
    File::create(path).ok().map(Mutex::new)
}

pub fn init(default_level: &str) {
    let setting = logs_setting(std::env::var("VICTORY_LOGS").ok().as_deref());
    console(setting);
    let file = if setting == Logs::Off {
        None
    } else {
        log_file()
    };
    let filter = std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level));
    let _ = tracing_subscriber::registry()
        .with(if setting == Logs::Off {
            LevelFilter::OFF
        } else {
            CAP
        })
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_writer(std::io::stderr),
        )
        .with(file.map(|file| {
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_writer(file)
        }))
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
        for other in [
            None,
            Some(""),
            Some("2"),
            Some("on"),
            Some("true"),
            Some("10"),
        ] {
            assert_eq!(logs_setting(other), Logs::Default, "{other:?}");
        }
    }

    #[test]
    fn the_oldest_logs_go_and_other_files_stay() {
        let dir = tempfile::tempdir().unwrap();
        for n in 1..=5 {
            std::fs::write(dir.path().join(format!("victory-{n:012}-1.log")), "x").unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), "keep").unwrap();
        prune(dir.path(), 3);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "notes.txt",
                "victory-000000000003-1.log",
                "victory-000000000004-1.log",
                "victory-000000000005-1.log",
            ]
        );
    }
}
