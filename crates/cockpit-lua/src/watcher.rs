//! Filesystem watcher that surfaces changed extension files (M9.5).
//!
//! The cockpit binary creates one [`ExtensionWatcher`] per loaded
//! extensions directory and polls [`ExtensionWatcher::changed`] during
//! its event loop. Each returned path is fed to [`LuaRuntime::reload`].
//! On platforms where `notify` cannot watch (rare — sandboxed CI),
//! [`ExtensionWatcher::start`] returns the watcher in a disabled state
//! and `changed` always reports empty.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{Event, EventKind, RecursiveMode, Watcher};

/// Cross-platform extension-file watcher.
pub struct ExtensionWatcher {
    receiver: Option<mpsc::Receiver<PathBuf>>,
    // Keep the watcher alive — dropping it cancels the watch.
    _watcher: Option<Box<dyn Watcher + Send + Sync>>,
}

impl std::fmt::Debug for ExtensionWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionWatcher")
            .field("enabled", &self.receiver.is_some())
            .finish()
    }
}

impl ExtensionWatcher {
    /// A disabled watcher — [`changed`](Self::changed) always returns
    /// empty. Used as a fallback when there is no extensions
    /// directory or `notify` cannot be initialised.
    pub fn disabled() -> Self {
        Self {
            receiver: None,
            _watcher: None,
        }
    }

    /// Start watching `dir` non-recursively for `*.lua` changes. The
    /// watcher runs on its own OS thread (managed by `notify`);
    /// channel polls are non-blocking.
    pub fn start(dir: impl AsRef<Path>) -> Result<Self, notify::Error> {
        Self::start_with_wake(dir, || {})
    }

    /// Start the watcher with an opaque `wake` callback that fires
    /// every time a Lua-related event lands in the channel. The cockpit
    /// binary uses this to nudge `winit`'s event loop so the reload
    /// runs on the next paint even when the window is idle.
    pub fn start_with_wake<F>(dir: impl AsRef<Path>, wake: F) -> Result<Self, notify::Error>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel::<PathBuf>();

        let watcher_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else {
                return;
            };
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_),
            ) {
                return;
            }
            let mut sent = false;
            for path in event.paths {
                if path.extension().and_then(|s| s.to_str()) == Some("lua") {
                    let _ = watcher_tx.send(path);
                    sent = true;
                }
            }
            if sent {
                wake();
            }
        })?;
        watcher.watch(dir.as_ref(), RecursiveMode::NonRecursive)?;
        Ok(Self {
            receiver: Some(rx),
            _watcher: Some(Box::new(watcher)),
        })
    }

    /// Try to read any extension paths that changed since the last
    /// poll. Returns an empty vec when the watcher is disabled or no
    /// events fired.
    pub fn changed(&self) -> Vec<PathBuf> {
        let Some(rx) = self.receiver.as_ref() else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        while let Ok(path) = rx.try_recv() {
            paths.push(path);
        }
        // De-duplicate consecutive duplicates so a single save doesn't
        // queue several reloads.
        paths.sort();
        paths.dedup();
        paths
    }
}

/// Convenience: poll repeatedly with a small back-off, returning the
/// first non-empty change set or `None` after `timeout`. Used by the
/// debounce path inside the binary's tick loop when a save is in
/// flight.
pub fn poll_with_timeout(watcher: &ExtensionWatcher, timeout: Duration) -> Option<Vec<PathBuf>> {
    let start = std::time::Instant::now();
    loop {
        let changes = watcher.changed();
        if !changes.is_empty() {
            return Some(changes);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_watcher_returns_empty() {
        let watcher = ExtensionWatcher::disabled();
        assert!(watcher.changed().is_empty());
    }

    #[test]
    fn poll_with_timeout_returns_none_on_silent_watcher() {
        let watcher = ExtensionWatcher::disabled();
        let result = poll_with_timeout(&watcher, Duration::from_millis(20));
        assert!(result.is_none());
    }
}
