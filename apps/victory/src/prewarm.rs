//! `victory prewarm`: the shaders go through the translator before a run, so
//! the title's first use finds them in the cache (decision 0003). Progress
//! shows in the console and in its title bar.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::shader::{MAX_FILE, sources};

/// How often the progress line changes.
const REFRESH: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Counts {
    done: AtomicUsize,
    translated: AtomicUsize,
    cached: AtomicUsize,
    refused: AtomicUsize,
}

/// The progress line, and the same text for the console's title bar.
fn line(counts: &Counts, total: usize, since: Instant) -> String {
    let done = counts.done.load(Ordering::Relaxed);
    let percent = done * 100 / total.max(1);
    let secs = since.elapsed().as_secs();
    format!(
        "shaders {done}/{total} ({percent}%): {} translated, {} already cached, {} refused, {}:{:02}",
        counts.translated.load(Ordering::Relaxed),
        counts.cached.load(Ordering::Relaxed),
        counts.refused.load(Ordering::Relaxed),
        secs / 60,
        secs % 60
    )
}

/// Translates every shader in `paths` (default: the game's `shaders.rpf`)
/// into the cache a run uses.
pub fn run(paths: Vec<PathBuf>, jobs: Option<usize>) -> Result<()> {
    let cfg = victory_core::Config::load()?;
    victory_hle::modules::d3d12x::use_shader_cache(cfg.cache_subdir("shaders")?);
    let files = if paths.is_empty() {
        vec![cfg.require_game_dir()?.join("shaders.rpf")]
    } else {
        paths
    };
    if !victory_hle::modules::d3d12x::prepare_shader_host() {
        bail!("no host GPU: translations made without one would not match a run's");
    }
    let jobs = jobs
        .or_else(|| std::thread::available_parallelism().ok().map(Into::into))
        .unwrap_or(4)
        .max(1);
    for file in files {
        let size = std::fs::metadata(&file)
            .with_context(|| file.display().to_string())?
            .len();
        if size > MAX_FILE {
            bail!("{} is larger than {MAX_FILE} bytes", file.display());
        }
        let data = std::fs::read(&file).with_context(|| file.display().to_string())?;
        let found = sources(&data);
        println!(
            "{}: {} shader containers, {jobs} threads",
            file.display(),
            found.len()
        );
        let counts = Counts::default();
        let next = AtomicUsize::new(0);
        let started = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..jobs {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(source) = found.get(i) else { return };
                        let outcome = source
                            .bytes()
                            .ok_or_else(|| "corrupt record".to_string())
                            .and_then(|bytes| victory_hle::modules::d3d12x::prewarm_shader(&bytes));
                        let tally = match outcome {
                            Ok(true) => &counts.translated,
                            Ok(false) => &counts.cached,
                            Err(_) => &counts.refused,
                        };
                        tally.fetch_add(1, Ordering::Relaxed);
                        counts.done.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
            while counts.done.load(Ordering::Relaxed) < found.len() {
                let text = line(&counts, found.len(), started);
                // The console title shows it too, with the window in the back.
                print!("\x1b]0;VIctory: {text}\x07\r{text}   ");
                let _ = std::io::stdout().flush();
                std::thread::sleep(REFRESH);
            }
        });
        let text = line(&counts, found.len(), started);
        println!("\x1b]0;VIctory: shaders ready\x07\r{text}   ");
    }
    println!("done: a run now finds these translations in the cache");
    Ok(())
}
