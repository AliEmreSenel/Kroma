//! Owned periodic workers that stop and release their state when dropped.

use std::{
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

/// Runs a periodic task until its owner is dropped, then joins the thread.
pub(crate) struct PeriodicWorker {
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl PeriodicWorker {
    /// Spawns a worker; waiting for the next tick is interruptible on shutdown.
    pub(crate) fn spawn(
        name: &str,
        interval: Duration,
        mut tick: impl FnMut() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (stop, receiver) = mpsc::channel();
        let thread = thread::Builder::new().name(name.into()).spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = receiver.recv_timeout(interval) {
                tick();
            }
        })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for PeriodicWorker {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("Background worker panicked during shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_worker_joins_and_releases_captured_state() {
        let (ticks, received) = mpsc::channel();
        let worker = PeriodicWorker::spawn("worker-test", Duration::from_millis(1), move || {
            let _ = ticks.send(());
        })
        .unwrap();
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(worker);
        // Drain any ticks already sent; disconnection proves the closure was dropped.
        while received.try_recv().is_ok() {}
        assert_eq!(received.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn shutdown_interrupts_a_long_tick_interval() {
        let worker =
            PeriodicWorker::spawn("worker-sleep-test", Duration::from_secs(60), || {}).unwrap();
        let start = std::time::Instant::now();
        drop(worker);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
