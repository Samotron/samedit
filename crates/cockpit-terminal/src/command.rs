//! A process command specification ready for the PTY layer to spawn.
//!
//! Lives at the crate root (rather than under `zellij`) because the embedded
//! multiplexer (v0.7 M7.9) spawns commands without going through any of the
//! Zellij command-construction helpers. The Zellij module continues to
//! re-export this type for now while its callers wind down.

/// A process command ready for the PTY layer to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    /// Create a command specification.
    pub fn new(program: impl Into<String>, args: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into(),
        }
    }
}
