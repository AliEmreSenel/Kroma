//! Opt-in timings for startup, compilation, and resource/lifecycle work.

use std::{sync::LazyLock, time::Instant};

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("KROMA_PROFILE_TIMINGS").is_ok_and(|value| value == "1"));

/// A stage timer, emitted only when `KROMA_PROFILE_TIMINGS=1`.
pub(crate) struct Scope {
    name: &'static str,
    start: Instant,
}

/// Starts an optional timing scope; normal runs avoid clock reads and stage logs.
pub(crate) fn scope(name: &'static str) -> Option<Scope> {
    ENABLED.then(|| Scope {
        name,
        start: Instant::now(),
    })
}

impl Drop for Scope {
    fn drop(&mut self) {
        log::info!(target: "kroma_profile", "stage={} elapsed_ms={:.3}",
            self.name, self.start.elapsed().as_secs_f64() * 1000.0);
    }
}
