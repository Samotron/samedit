//! Materialisations — M5.8.
//!
//! `Models: Build` walks the DAG in topological order and turns each
//! model into one [`BuildStep`]. View and table materialisations get
//! `CREATE OR REPLACE` statements; ephemeral models contribute a CTE
//! binding that gets inlined into every downstream model's rendered
//! SQL (the [`render_for_engine`] helper does the inlining).
//!
//! The output is pure SQL — callers execute it through the M5.1
//! `SqlEngine`. The notebook UI surfaces a `Models: Build All` /
//! `Models: Build Selected` palette entry that drives this.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::dag::{DagError, ModelDag};
use crate::detect::{AnalyticsProject, Materialisation, Model};
use crate::template::{StaticResolver, TemplateError, render_model};

/// One materialisation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep {
    /// Model the step targets.
    pub model: String,
    /// Effective materialisation (after model-level overrides).
    pub materialisation: Materialisation,
    /// SQL statement to feed to the engine. Empty for
    /// [`Materialisation::Ephemeral`] — those have no on-disk
    /// statement, only a CTE binding inlined into dependents.
    pub statement: String,
}

/// Full build plan: every step, in topological order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub steps: Vec<BuildStep>,
}

/// Errors that can stop a plan from being assembled.
#[derive(Debug, Error)]
pub enum BuildError {
    /// One of the models contains a templating error (unknown ref,
    /// malformed expression).
    #[error("template error in {model}: {error}")]
    Template { model: String, error: TemplateError },
    /// The DAG had a cycle — see [`DagError::Cycle`] for the offending
    /// names.
    #[error("{0}")]
    Dag(#[from] DagError),
}

/// Build the materialisation plan for `project`. The plan respects the
/// configured materialisation per model and inlines every upstream
/// ephemeral model as a CTE in dependent statements.
pub fn build_plan(project: &AnalyticsProject) -> Result<BuildPlan, BuildError> {
    let dag = ModelDag::from_models(&project.models);
    let order = dag.topological_order()?;
    let models_by_name: BTreeMap<&str, &Model> = project
        .models
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();

    let resolver = StaticResolver::new().with_models(
        project
            .models
            .iter()
            .map(|m| (m.name.as_str(), m.materialisation)),
    );

    // Precompute each ephemeral model's rendered body so we can splice
    // it into downstream statements as a CTE without redoing the work.
    let mut ephemeral_bodies: BTreeMap<String, String> = BTreeMap::new();
    for model in &project.models {
        if model.materialisation != Materialisation::Ephemeral {
            continue;
        }
        let body = render_model(&model.source, &resolver).map_err(|err| BuildError::Template {
            model: model.name.clone(),
            error: err,
        })?;
        ephemeral_bodies.insert(model.name.clone(), body);
    }

    let mut steps = Vec::new();
    for name in order {
        let model = match models_by_name.get(name.as_str()) {
            Some(m) => *m,
            None => continue,
        };
        if model.materialisation == Materialisation::Ephemeral {
            // No on-disk step; the CTE binding lands in dependents.
            steps.push(BuildStep {
                model: model.name.clone(),
                materialisation: model.materialisation,
                statement: String::new(),
            });
            continue;
        }
        let rendered =
            render_model(&model.source, &resolver).map_err(|err| BuildError::Template {
                model: model.name.clone(),
                error: err,
            })?;
        let upstream = upstream_ephemerals(model, &project.models, &ephemeral_bodies);
        let statement =
            wrap_materialisation(model.materialisation, &model.name, &rendered, &upstream);
        steps.push(BuildStep {
            model: model.name.clone(),
            materialisation: model.materialisation,
            statement,
        });
    }

    Ok(BuildPlan { steps })
}

/// Collect every ephemeral model `model` transitively depends on so the
/// plan can prepend them as CTEs. Returns `(alias, body)` pairs in the
/// order they should appear in the resulting `WITH` clause.
fn upstream_ephemerals(
    model: &Model,
    models: &[Model],
    ephemeral_bodies: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let by_name: BTreeMap<&str, &Model> = models.iter().map(|m| (m.name.as_str(), m)).collect();
    let mut visited: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut stack = vec![model.name.clone()];
    let mut out: Vec<(String, String)> = Vec::new();
    let mut already: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    while let Some(current) = stack.pop() {
        if visited.contains_key(&current) {
            continue;
        }
        let Some(node) = by_name.get(current.as_str()) else {
            continue;
        };
        let deps = crate::dag::ModelDag::from_models(models)
            .node(&node.name)
            .map(|n| n.dependencies.clone())
            .unwrap_or_default();
        visited.insert(current.clone(), Vec::new());
        for dep in deps.iter() {
            stack.push(dep.clone());
            if let Some(body) = ephemeral_bodies.get(dep)
                && already.insert(dep.clone())
            {
                out.push((format!("__cockpit_eph_{dep}"), body.clone()));
            }
        }
    }
    // Reverse so deepest ephemerals land first — that way an ephemeral
    // model that itself references another ephemeral still resolves.
    out.reverse();
    out
}

fn wrap_materialisation(
    materialisation: Materialisation,
    name: &str,
    body: &str,
    upstream: &[(String, String)],
) -> String {
    let body_with_ctes = if upstream.is_empty() {
        body.trim().to_string()
    } else {
        let mut text = String::from("WITH ");
        for (i, (alias, alias_body)) in upstream.iter().enumerate() {
            if i > 0 {
                text.push_str(",\n     ");
            }
            text.push_str(alias);
            text.push_str(" AS (\n");
            text.push_str(alias_body.trim());
            text.push_str("\n     )");
        }
        text.push('\n');
        text.push_str(body.trim());
        text
    };
    match materialisation {
        Materialisation::View => format!("CREATE OR REPLACE VIEW {name} AS\n{body_with_ctes};"),
        Materialisation::Table => format!("CREATE OR REPLACE TABLE {name} AS\n{body_with_ctes};"),
        // Should not reach here — ephemeral handled by the caller.
        Materialisation::Ephemeral => body_with_ctes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AnalyticsConfig;
    use std::path::PathBuf;

    fn project(models: Vec<Model>) -> AnalyticsProject {
        AnalyticsProject {
            root_path: PathBuf::from("/proj"),
            config: AnalyticsConfig::default(),
            models,
        }
    }

    fn model(name: &str, source: &str, mat: Materialisation) -> Model {
        Model {
            name: name.to_string(),
            path: PathBuf::from(format!("models/{name}.sql")),
            source: source.to_string(),
            materialisation: mat,
        }
    }

    #[test]
    fn view_models_become_create_or_replace_view_statements() {
        let plan = build_plan(&project(vec![model(
            "stg_orders",
            "SELECT 1 AS id",
            Materialisation::View,
        )]))
        .unwrap();
        assert_eq!(plan.steps.len(), 1);
        let step = &plan.steps[0];
        assert_eq!(step.materialisation, Materialisation::View);
        assert!(step.statement.contains("CREATE OR REPLACE VIEW stg_orders"));
        assert!(step.statement.contains("SELECT 1 AS id"));
        assert!(step.statement.ends_with(';'));
    }

    #[test]
    fn table_models_become_create_or_replace_table_statements() {
        let plan = build_plan(&project(vec![model(
            "fct_orders",
            "SELECT 1 AS id",
            Materialisation::Table,
        )]))
        .unwrap();
        assert!(
            plan.steps[0]
                .statement
                .contains("CREATE OR REPLACE TABLE fct_orders")
        );
    }

    #[test]
    fn ephemeral_models_inline_as_ctes_in_downstream_statements() {
        let plan = build_plan(&project(vec![
            model("raw", "SELECT 1 AS id", Materialisation::Ephemeral),
            model(
                "stg_orders",
                "SELECT * FROM {{ ref('raw') }}",
                Materialisation::View,
            ),
        ]))
        .unwrap();
        // The ephemeral model has an empty statement — nothing to run.
        let raw_step = plan
            .steps
            .iter()
            .find(|s| s.model == "raw")
            .expect("raw step exists");
        assert!(raw_step.statement.is_empty());
        // The downstream view's statement contains the CTE binding and
        // references the alias rather than the raw model name.
        let stg_step = plan
            .steps
            .iter()
            .find(|s| s.model == "stg_orders")
            .expect("stg step exists");
        assert!(stg_step.statement.contains("WITH __cockpit_eph_raw AS ("));
        assert!(stg_step.statement.contains("FROM __cockpit_eph_raw"));
    }

    #[test]
    fn unknown_refs_surface_a_template_error() {
        let err = build_plan(&project(vec![model(
            "broken",
            "SELECT * FROM {{ ref('missing') }}",
            Materialisation::View,
        )]))
        .unwrap_err();
        match err {
            BuildError::Template { model, error } => {
                assert_eq!(model, "broken");
                assert_eq!(error, TemplateError::UnknownRef("missing".to_string()));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn cycles_surface_a_dag_error() {
        let err = build_plan(&project(vec![
            model("a", "SELECT * FROM {{ ref('b') }}", Materialisation::View),
            model("b", "SELECT * FROM {{ ref('a') }}", Materialisation::View),
        ]))
        .unwrap_err();
        assert!(matches!(err, BuildError::Dag(DagError::Cycle(_))));
    }
}
