//! Cold-start hydration driver — v0.6 M6.2.
//!
//! Owns the per-project bring-up work (`detect → load_tree → build_model
//! → apply_config → refresh_git → restore_cache`) as a state machine that
//! advances exactly one phase per call to [`HydrationDriver::advance`].
//! The render harness drives this from its post-paint
//! [`cockpit_render::CockpitApp::tick`] callback so the splash appears on
//! frame 1 and the first heavy phase only runs once that frame is on
//! screen.
//!
//! The driver lives in the binary (not in `cockpit-ui`) because the
//! intermediate data it carries — [`ProjectDetection`], [`FileTree`],
//! [`AppModel`] — is rooted in `cockpit-project` and friends. Splash
//! progress reporting is delegated to the headless
//! [`cockpit_ui::HydrationProgress`] view-model.

use std::mem;
use std::path::PathBuf;
use std::time::Instant;

use cockpit_project::{
    FileTree, ProjectDetection, RecentProjects, detect_project, recent_projects_path,
};
use cockpit_ui::{HydrationPhase, HydrationProgress};

use crate::app::AppModel;
use crate::startup;

/// What the harness should do after one call to
/// [`HydrationDriver::advance`].
///
/// `Ready` boxes the model so the enum stays small — `AppModel` carries
/// a couple of kilobytes of pane / palette / LSP state, and we don't
/// want every `advance` return to copy that on the stack.
pub enum HydrationOutcome {
    /// More phases queued; advance again on the next tick.
    Continue,
    /// Every phase succeeded; the produced model is ready to drive the
    /// live UI.
    Ready(Box<AppModel>),
    /// A phase failed irrecoverably. The driver stays in the failed
    /// state and subsequent `advance` calls return [`HydrationOutcome::Failed`]
    /// with the same message. The shell keeps painting the splash so the
    /// user can read the error and close the window.
    Failed(String),
}

/// Internal driver state. Each variant owns exactly the data the *next*
/// phase needs, so `advance` is a `mem::replace` + match.
enum DriverState {
    NotStarted {
        path: PathBuf,
    },
    Detected {
        path: PathBuf,
        detection: ProjectDetection,
    },
    TreeLoaded {
        detection: ProjectDetection,
        tree: FileTree,
    },
    ModelBuilt {
        model: AppModel,
    },
    ConfigApplied {
        model: AppModel,
    },
    GitRefreshed {
        model: AppModel,
    },
    Done {
        model: AppModel,
    },
    Failed {
        message: String,
    },
    /// Transient placeholder so `mem::replace` has something to move out
    /// of. Never observable outside `run_phase`.
    Transitioning,
}

/// Cold-start hydration state machine.
pub struct HydrationDriver {
    state: DriverState,
    progress: HydrationProgress,
}

impl HydrationDriver {
    /// Build a driver that will hydrate the project at `path`.
    pub fn new(path: PathBuf) -> Self {
        Self {
            state: DriverState::NotStarted { path },
            progress: HydrationProgress::default_phases(),
        }
    }

    /// Read-only view-model used by the splash painter.
    pub fn progress(&self) -> &HydrationProgress {
        &self.progress
    }

    /// Run exactly one phase. After `Ready` or `Failed`, further calls
    /// return [`HydrationOutcome::Continue`] only if there is residual
    /// work — for terminal states the result mirrors the latest one.
    pub fn advance(&mut self) -> HydrationOutcome {
        let Some(phase) = self.progress.begin_next() else {
            return self.terminal_outcome();
        };

        let start = Instant::now();
        let result = self.run_phase(phase);
        let elapsed = start.elapsed();
        startup::record(phase.span(), elapsed);

        match result {
            Ok(()) => {
                self.progress.complete_current(elapsed);
                self.terminal_outcome()
            }
            Err(message) => {
                self.progress.fail(message.clone());
                self.state = DriverState::Failed {
                    message: message.clone(),
                };
                HydrationOutcome::Failed(message)
            }
        }
    }

    /// If the driver has reached a terminal state, return the matching
    /// outcome (taking ownership of the model on the `Done` path).
    /// Otherwise report `Continue`.
    fn terminal_outcome(&mut self) -> HydrationOutcome {
        match &self.state {
            DriverState::Done { .. } => {
                let prev = mem::replace(&mut self.state, DriverState::Transitioning);
                let DriverState::Done { model } = prev else {
                    unreachable!("matched Done above")
                };
                HydrationOutcome::Ready(Box::new(model))
            }
            DriverState::Failed { message } => HydrationOutcome::Failed(message.clone()),
            _ => HydrationOutcome::Continue,
        }
    }

    /// Execute `phase`, advancing the driver's owned data to the next
    /// variant. Returns the error message verbatim if the phase fails;
    /// the caller is responsible for routing it through `progress.fail`.
    fn run_phase(&mut self, phase: HydrationPhase) -> Result<(), String> {
        let prev = mem::replace(&mut self.state, DriverState::Transitioning);
        let next = match (phase, prev) {
            (HydrationPhase::Detect, DriverState::NotStarted { path }) => {
                let detection = detect_project(&path)
                    .map_err(|err| format!("detect project at `{}`: {err}", path.display()))?;
                record_recent_project(&detection);
                DriverState::Detected { path, detection }
            }
            (HydrationPhase::LoadTree, DriverState::Detected { path, detection }) => {
                let tree = FileTree::load(&path)
                    .map_err(|err| format!("load file tree at `{}`: {err}", path.display()))?;
                DriverState::TreeLoaded { detection, tree }
            }
            (HydrationPhase::BuildModel, DriverState::TreeLoaded { detection, tree }) => {
                let model = AppModel::new(detection, tree)?;
                DriverState::ModelBuilt { model }
            }
            (HydrationPhase::ApplyConfig, DriverState::ModelBuilt { mut model }) => {
                if let Some(path) = cockpit_config::user_config_path() {
                    match cockpit_config::Config::load_optional(&path) {
                        Ok(config) => {
                            model.apply_user_config(&config);
                            model.set_user_config_path(path);
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "user config load failed");
                        }
                    }
                }
                // v0.9 M9: load Lua extensions on the same phase so
                // their registrations land before the user lifts a
                // finger. The directory is derived from the user
                // config path; embedded defaults always load.
                let ext_dir = cockpit_config::user_config_path()
                    .and_then(|p| p.parent().map(|d| d.join("extensions")));
                model.load_lua_extensions(ext_dir.as_deref());
                DriverState::ConfigApplied { model }
            }
            (HydrationPhase::RefreshGit, DriverState::ConfigApplied { mut model }) => {
                model.refresh_git_status();
                DriverState::GitRefreshed { model }
            }
            (HydrationPhase::RestoreCache, DriverState::GitRefreshed { mut model }) => {
                model.restore_cached_state();
                DriverState::Done { model }
            }
            (phase, state) => {
                // Defensive: only fires if the phase queue and state
                // machine drift apart. Restore the prior state so the
                // splash can render the error without losing data.
                self.state = state;
                return Err(format!(
                    "hydration phase `{}` reached an unexpected driver state",
                    phase.label()
                ));
            }
        };
        self.state = next;
        Ok(())
    }
}

/// Add a project to the launcher's recent-projects registry. Best-effort:
/// a cache failure must never stop the project from opening.
fn record_recent_project(detection: &ProjectDetection) {
    let Ok(path) = recent_projects_path() else {
        return;
    };
    let mut recents = RecentProjects::load(&path).unwrap_or_default();
    recents.record(&detection.root_path, &detection.display_name);
    if let Err(err) = recents.store(&path) {
        tracing::warn!(error = %err, "failed to store recent projects");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_testkit::fixture_path;

    /// Drive the full happy-path through every phase against a real fixture
    /// and assert the produced model knows the project's display name.
    /// The terminal `Ready` arrives on the same tick that completes the
    /// final phase — `terminal_outcome` consumes `Done` immediately rather
    /// than holding it for an extra advance call.
    #[test]
    fn advance_walks_every_phase_to_a_live_model() {
        let mut driver = HydrationDriver::new(fixture_path("rust-basic"));
        let mut model = None;

        for expected in HydrationPhase::ALL {
            assert!(model.is_none(), "Ready arrived before the last phase");
            match driver.advance() {
                HydrationOutcome::Continue => {}
                HydrationOutcome::Ready(produced) => model = Some(*produced),
                HydrationOutcome::Failed(message) => {
                    panic!("phase {:?} reported a failure: {}", expected, message)
                }
            }
            let completed = driver.progress().completed();
            assert!(
                completed.iter().any(|c| c.phase == expected),
                "phase {:?} did not appear in completed list",
                expected,
            );
        }

        let model = model.expect("the last phase should have produced Ready");
        assert_eq!(model.project_name(), "rust-basic");
        assert!(driver.progress().is_done());
    }

    /// A nonexistent path fails partway through hydration and leaves the
    /// driver in a stable failed state. `detect_project` itself is happy
    /// to enumerate signals on a missing directory — the actual fault
    /// surfaces when the file-tree walk hits the missing path.
    #[test]
    fn missing_path_fails_during_tree_load_and_stays_failed() {
        let bogus = std::path::PathBuf::from("/this/path/should/not/exist/anywhere");
        let mut driver = HydrationDriver::new(bogus);

        // Run until either Ready (unexpected) or Failed bubbles up.
        let mut last = HydrationOutcome::Continue;
        for _ in 0..HydrationPhase::ALL.len() + 1 {
            last = driver.advance();
            if matches!(last, HydrationOutcome::Failed(_)) {
                break;
            }
        }

        let message = match last {
            HydrationOutcome::Failed(msg) => msg,
            HydrationOutcome::Ready(_) => panic!("missing path must not produce a live model"),
            HydrationOutcome::Continue => panic!("hydration never reached a terminal state"),
        };
        assert!(
            message.contains("file tree") || message.contains("detect"),
            "unexpected failure message: {message}",
        );
        assert!(driver.progress().is_failed());

        // Subsequent advances are sticky: still failed, same message.
        match driver.advance() {
            HydrationOutcome::Failed(again) => assert_eq!(again, message),
            _ => panic!("driver should remain in failed state"),
        }
    }
}
