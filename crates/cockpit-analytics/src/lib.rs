//! `cockpit-analytics` — dbt-lite project mode (v0.5 M5.6–M5.9).
//!
//! Spec posture (plan §8a):
//!
//! * Detect a `models/` directory + `cockpit-analytics.toml` (or a
//!   `[metadata.cockpit.analytics]` block in `mise.toml`) and treat the
//!   project as a dbt-lite analytics project. M5.6.
//! * Minimal Jinja subset: `{{ ref('name') }}` and `{{ source('schema',
//!   'table') }}` are the only expressions we resolve. Hand-rolled —
//!   no full Jinja crate. M5.7.
//! * Materialisations: `view`, `table`, `ephemeral` (CTE-inlined). M5.8.
//! * Read-time DAG: re-parsed on save, never indexed in the background
//!   (respects spec §3.9 / §24). M5.9.
//!
//! Headless: every module exposes pure functions over plain data so
//! tests can drive the whole layer with `cockpit-project::FakeFileSystem`
//! (M4.10).

pub mod dag;
pub mod detect;
pub mod materialise;
pub mod template;

pub use dag::{ModelDag, ModelNode};
pub use detect::{
    AnalyticsConfig, AnalyticsProject, Materialisation, Model, detect_analytics_project,
};
pub use materialise::{BuildPlan, BuildStep, build_plan};
pub use template::{TemplateError, TemplateResolver, render_model};
