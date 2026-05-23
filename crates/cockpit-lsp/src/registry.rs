//! Language → server-command registry.
//!
//! The single seam where adding a language means adding a row. Stays pure so
//! tests assert on the table without spawning anything; the binary wraps the
//! command in `mise exec` separately so cockpit never bypasses the project
//! environment (spec §19).

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
            Language::Rust => Some(Self {
                language,
                command: "rust-analyzer".to_string(),
                args: Vec::new(),
            }),
        }
    }

    /// The LSP `languageId` string the server expects in `textDocument` items
    /// (e.g. `"rust"` for rust-analyzer).
    pub fn language_id(&self) -> &'static str {
        match self.language {
            Language::Rust => "rust",
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
}
