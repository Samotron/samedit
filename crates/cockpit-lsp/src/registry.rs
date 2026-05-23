//! Language → server-command registry.
//!
//! The single seam where adding a language means adding a row. Stays pure so
//! tests assert on the table without spawning anything; the binary wraps the
//! command in `mise exec` separately so cockpit never bypasses the project
//! environment (spec §19). Syntax highlighting can lag this table; unsupported
//! highlighters simply return no spans while LSP still starts for recognised
//! source files.

use cockpit_editor::Language;

/// How to launch the language server for one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Language this server serves.
    pub language: Language,
    /// The server binary (looked up on PATH, or whatever `mise exec` resolves).
    pub command: String,
    /// Extra arguments after the binary.
    pub args: Vec<String>,
}

impl ServerConfig {
    /// The configured language server for `language`, or `None` when there is
    /// no entry for it yet.
    pub fn for_language(language: Language) -> Option<Self> {
        match language {
            Language::Python => Some(Self {
                language,
                command: "basedpyright-langserver".to_string(),
                args: vec!["--stdio".to_string()],
            }),
            Language::Rust => Some(Self {
                language,
                command: "rust-analyzer".to_string(),
                args: Vec::new(),
            }),
            Language::Sql => Some(Self {
                language,
                command: "sqls".to_string(),
                args: Vec::new(),
            }),
            Language::TypeScript => Some(Self {
                language,
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
            }),
            // ggsql has no dedicated language server yet (v0.5 M5.5a) —
            // notebook cells fall back to sqls when they need schema
            // intelligence, since ggsql wraps DuckDB anyway.
            Language::Ggsql => None,
        }
    }

    /// The LSP `languageId` string the server expects in `textDocument` items
    /// (e.g. `"rust"` for rust-analyzer).
    pub fn language_id(&self) -> &'static str {
        match self.language {
            Language::Python => "python",
            Language::Rust => "rust",
            Language::Sql => "sql",
            Language::Ggsql => "ggsql",
            Language::TypeScript => "typescript",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_resolves_to_rust_analyzer() {
        let config = ServerConfig::for_language(Language::Rust).expect("rust has a server");
        assert_eq!(config.language, Language::Rust);
        assert_eq!(config.command, "rust-analyzer");
        assert!(config.args.is_empty());
        assert_eq!(config.language_id(), "rust");
    }

    #[test]
    fn v04_lsp_languages_resolve_to_servers() {
        let cases = [
            (
                Language::TypeScript,
                "typescript-language-server",
                &["--stdio"][..],
                "typescript",
            ),
            (
                Language::Python,
                "basedpyright-langserver",
                &["--stdio"][..],
                "python",
            ),
            (Language::Sql, "sqls", &[][..], "sql"),
        ];

        for (language, command, args, language_id) in cases {
            let config = ServerConfig::for_language(language).expect("language has a server");
            assert_eq!(config.command, command);
            assert_eq!(config.args, args);
            assert_eq!(config.language_id(), language_id);
        }
    }
}
