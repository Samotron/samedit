//! Minimal Jinja-subset templating — M5.7.
//!
//! dbt's `{{ ref('name') }}` and `{{ source('schema', 'table') }}`
//! syntax is the only thing we resolve. Anything else (loops,
//! conditionals, filters) is out of scope: cockpit's analytics mode
//! is dbt-*lite*, not dbt.
//!
//! Resolution is purely textual. `{{ ref('stg_orders') }}` becomes
//! whatever identifier the [`TemplateResolver`] decides on — for view
//! and table materialisations that's just the model name; for
//! ephemeral models it's a CTE binding produced upstream (see
//! [`crate::materialise::build_plan`]).

use std::collections::HashMap;

use thiserror::Error;

use crate::detect::Materialisation;

/// Resolves `ref(...)` and `source(...)` calls to concrete identifiers.
pub trait TemplateResolver {
    /// Identifier for the model `name`. Returns `None` when the model
    /// is unknown so the renderer can raise [`TemplateError::UnknownRef`].
    fn resolve_ref(&self, name: &str) -> Option<String>;
    /// Identifier for `source(schema, table)`. Returns `None` when the
    /// source is unknown.
    fn resolve_source(&self, schema: &str, table: &str) -> Option<String>;
}

/// Lookup table resolver — the implementation cockpit's analytics
/// builder uses. Maps each model to its materialised identifier and
/// each source to its `<schema>.<table>` SQL form.
#[derive(Debug, Clone, Default)]
pub struct StaticResolver {
    refs: HashMap<String, String>,
    sources: HashMap<(String, String), String>,
}

impl StaticResolver {
    /// New empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `name` to `identifier`. The identifier is inserted verbatim
    /// into the rendered SQL — quote and qualify it as needed.
    pub fn with_ref(mut self, name: impl Into<String>, identifier: impl Into<String>) -> Self {
        self.refs.insert(name.into(), identifier.into());
        self
    }

    /// Bind `(schema, table)` to a source identifier.
    pub fn with_source(
        mut self,
        schema: impl Into<String>,
        table: impl Into<String>,
        identifier: impl Into<String>,
    ) -> Self {
        self.sources
            .insert((schema.into(), table.into()), identifier.into());
        self
    }

    /// Default `ref(name)` resolution given each model's
    /// materialisation. View / table models reference their own name;
    /// ephemeral models reference a CTE alias of the form
    /// `__cockpit_eph_<name>` — [`crate::materialise::build_plan`]
    /// emits matching `WITH __cockpit_eph_<name> AS (...)` clauses.
    pub fn with_models<'a, I>(mut self, models: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, Materialisation)>,
    {
        for (name, mat) in models {
            let identifier = match mat {
                Materialisation::View | Materialisation::Table => name.to_string(),
                Materialisation::Ephemeral => format!("__cockpit_eph_{name}"),
            };
            self.refs.insert(name.to_string(), identifier);
        }
        self
    }
}

impl TemplateResolver for StaticResolver {
    fn resolve_ref(&self, name: &str) -> Option<String> {
        self.refs.get(name).cloned()
    }
    fn resolve_source(&self, schema: &str, table: &str) -> Option<String> {
        self.sources
            .get(&(schema.to_string(), table.to_string()))
            .cloned()
    }
}

/// Things that can go wrong while rendering a model.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    /// `{{ ref('unknown') }}` — the resolver returned None.
    #[error("unknown ref `{0}`")]
    UnknownRef(String),
    /// `{{ source('schema', 'unknown') }}` — the resolver returned None.
    #[error("unknown source `{schema}.{table}`")]
    UnknownSource { schema: String, table: String },
    /// Malformed `{{ ... }}` expression we couldn't parse.
    #[error("malformed template expression: {0}")]
    Malformed(String),
}

/// Resolve every `{{ ref(...) }}` and `{{ source(...) }}` in `source`,
/// returning the rendered SQL. Non-`ref`/`source` `{{ ... }}` blocks
/// raise [`TemplateError::Malformed`] — failing loudly beats silently
/// passing dbt-specific Jinja through to DuckDB.
pub fn render_model(
    source: &str,
    resolver: &dyn TemplateResolver,
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0;
    let bytes = source.as_bytes();
    while cursor < bytes.len() {
        if cursor + 1 < bytes.len() && &bytes[cursor..cursor + 2] == b"{{" {
            let Some(end) = find_close(&source[cursor + 2..]) else {
                return Err(TemplateError::Malformed(source[cursor..].to_string()));
            };
            let expr = &source[cursor + 2..cursor + 2 + end];
            let resolved = resolve_expression(expr.trim(), resolver)?;
            out.push_str(&resolved);
            cursor += 2 + end + 2; // skip `}}`
            continue;
        }
        // Push the byte verbatim; works because `{{` is two ASCII bytes
        // so we never split a UTF-8 codepoint.
        out.push(bytes[cursor] as char);
        cursor += 1;
    }
    Ok(out)
}

fn find_close(rest: &str) -> Option<usize> {
    rest.find("}}")
}

fn resolve_expression(
    expr: &str,
    resolver: &dyn TemplateResolver,
) -> Result<String, TemplateError> {
    if let Some(args) = expr.strip_prefix("ref(").and_then(|s| s.strip_suffix(')')) {
        let name =
            parse_string_arg(args).ok_or_else(|| TemplateError::Malformed(expr.to_string()))?;
        return resolver
            .resolve_ref(&name)
            .ok_or(TemplateError::UnknownRef(name));
    }
    if let Some(args) = expr
        .strip_prefix("source(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let (schema, table) =
            parse_two_args(args).ok_or_else(|| TemplateError::Malformed(expr.to_string()))?;
        return resolver
            .resolve_source(&schema, &table)
            .ok_or(TemplateError::UnknownSource { schema, table });
    }
    Err(TemplateError::Malformed(expr.to_string()))
}

/// Parse a single `"foo"` (or `'foo'`) argument. Whitespace inside is
/// preserved; the wrapper layer trims as needed.
fn parse_string_arg(args: &str) -> Option<String> {
    let trimmed = args.trim();
    strip_quotes(trimmed)
}

fn parse_two_args(args: &str) -> Option<(String, String)> {
    let mut parts = args.split(',');
    let first = parts.next()?.trim();
    let second = parts.next()?.trim();
    if parts.next().is_some() {
        return None;
    }
    Some((strip_quotes(first)?, strip_quotes(second)?))
}

fn strip_quotes(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let (open, close) = (trimmed.chars().next()?, trimmed.chars().last()?);
    if open != close || (open != '"' && open != '\'') {
        return None;
    }
    let inner = &trimmed[open.len_utf8()..trimmed.len() - close.len_utf8()];
    Some(inner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> StaticResolver {
        StaticResolver::new()
            .with_ref("stg_orders", "stg_orders")
            .with_ref("fct_orders", "fct_orders")
            .with_ref("eph_orders", "__cockpit_eph_eph_orders")
            .with_source("raw", "orders", "raw.orders")
    }

    #[test]
    fn ref_substitution_replaces_with_resolver_value() {
        let rendered = render_model(
            "SELECT * FROM {{ ref('stg_orders') }} WHERE n > 0",
            &resolver(),
        )
        .unwrap();
        assert_eq!(rendered, "SELECT * FROM stg_orders WHERE n > 0");
    }

    #[test]
    fn source_substitution_uses_two_arg_lookup() {
        let rendered =
            render_model("SELECT * FROM {{ source('raw', 'orders') }}", &resolver()).unwrap();
        assert_eq!(rendered, "SELECT * FROM raw.orders");
    }

    #[test]
    fn ephemeral_refs_use_cte_alias() {
        let rendered = render_model("SELECT * FROM {{ ref('eph_orders') }}", &resolver()).unwrap();
        assert_eq!(rendered, "SELECT * FROM __cockpit_eph_eph_orders");
    }

    #[test]
    fn unknown_ref_is_an_error() {
        let err = render_model("{{ ref('missing') }}", &resolver()).unwrap_err();
        assert_eq!(err, TemplateError::UnknownRef("missing".to_string()));
    }

    #[test]
    fn unknown_source_is_an_error() {
        let err = render_model("{{ source('raw', 'missing') }}", &resolver()).unwrap_err();
        assert_eq!(
            err,
            TemplateError::UnknownSource {
                schema: "raw".to_string(),
                table: "missing".to_string()
            }
        );
    }

    #[test]
    fn malformed_expression_is_an_error() {
        // Unterminated `}}`.
        let err = render_model("SELECT {{ ref('x'", &resolver()).unwrap_err();
        assert!(matches!(err, TemplateError::Malformed(_)));

        // Unknown function.
        let err = render_model("{{ env_var('FOO') }}", &resolver()).unwrap_err();
        assert!(matches!(err, TemplateError::Malformed(_)));
    }

    #[test]
    fn templates_can_appear_multiple_times_in_one_model() {
        let rendered = render_model(
            "SELECT s.x, o.y\nFROM {{ ref('stg_orders') }} s\nJOIN {{ ref('fct_orders') }} o ON s.id = o.id",
            &resolver(),
        )
        .unwrap();
        assert!(rendered.contains("FROM stg_orders s"));
        assert!(rendered.contains("JOIN fct_orders o"));
    }

    #[test]
    fn with_models_helper_seeds_refs_from_materialisations() {
        let resolver = StaticResolver::new().with_models([
            ("view_model", Materialisation::View),
            ("table_model", Materialisation::Table),
            ("eph_model", Materialisation::Ephemeral),
        ]);
        assert_eq!(
            resolver.resolve_ref("view_model"),
            Some("view_model".to_string())
        );
        assert_eq!(
            resolver.resolve_ref("table_model"),
            Some("table_model".to_string())
        );
        assert_eq!(
            resolver.resolve_ref("eph_model"),
            Some("__cockpit_eph_eph_model".to_string())
        );
    }
}
