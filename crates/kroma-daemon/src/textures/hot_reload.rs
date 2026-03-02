//! Shared filesystem hot-reload watcher for external texture assets.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Optional filesystem watcher bound to a single source file.
pub struct SourceHotReload {
    source_path: Option<PathBuf>,
    _watch_target: Option<PathBuf>,
    _watcher: Option<RecommendedWatcher>,
    watch_rx: Option<mpsc::Receiver<notify::Result<notify::Event>>>,
}

impl SourceHotReload {
    /// Construct a disabled watcher.
    pub fn disabled() -> Self {
        Self {
            source_path: None,
            _watch_target: None,
            _watcher: None,
            watch_rx: None,
        }
    }

    /// Create a watcher for a specific source path.
    pub fn from_source(source_path: Option<&Path>) -> Result<Self> {
        let Some(source_path) = source_path else {
            return Ok(Self::disabled());
        };

        let canonical_source = source_path
            .canonicalize()
            .unwrap_or_else(|_| source_path.to_path_buf());

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;

        let watch_target = if canonical_source.is_file() {
            canonical_source
                .parent()
                .unwrap_or(&canonical_source)
                .to_path_buf()
        } else {
            canonical_source.clone()
        };

        watcher.watch(&watch_target, RecursiveMode::NonRecursive)?;

        Ok(Self {
            source_path: Some(canonical_source),
            _watch_target: Some(watch_target),
            _watcher: Some(watcher),
            watch_rx: Some(rx),
        })
    }

    /// Source file path being watched, if enabled.
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// Return the source path when relevant file changes were observed.
    pub fn take_changed_path(&mut self) -> Option<PathBuf> {
        let Some(rx) = self.watch_rx.as_ref() else {
            return None;
        };
        let Some(source_path) = self.source_path.as_ref() else {
            return None;
        };

        let mut changed = false;

        while let Ok(evt) = rx.try_recv() {
            match evt {
                Ok(event) => {
                    let is_relevant_kind = matches!(
                        event.kind,
                        notify::EventKind::Modify(_)
                            | notify::EventKind::Create(_)
                            | notify::EventKind::Remove(_)
                            | notify::EventKind::Any
                    );

                    if !is_relevant_kind {
                        continue;
                    }

                    if event.paths.is_empty() {
                        changed = true;
                        continue;
                    }

                    for event_path in event.paths {
                        if event_path == *source_path {
                            changed = true;
                            break;
                        }
                        if let Ok(canon) = event_path.canonicalize()
                            && canon == *source_path
                        {
                            changed = true;
                            break;
                        }
                    }
                }
                Err(_) => {
                    changed = true;
                }
            }
        }

        if changed {
            Some(source_path.clone())
        } else {
            None
        }
    }
}
