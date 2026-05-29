//! Disk/config glue for `cockpit-quick`: load `launcher.toml`, expand project
//! paths, and assemble a [`Launcher`] from the enabled providers.
//!
//! Kept separate from the [`controller`](crate::controller) so the controller
//! stays pure. The filesystem is injected (`cockpit-project::env`), so the
//! assembly is testable without touching real disk.

use std::path::{Path, PathBuf};

use cockpit_config::{LauncherConfig, LauncherPosition};
use cockpit_launcher::{
    CalculatorProvider, Launcher, LuaAction, LuaActionsProvider, MiseTasksProvider, OrgProvider,
    RecentProjectsProvider, ThemeProvider, UrlProvider,
};
use cockpit_project::env::FileSystem;

/// Inputs the binary gathers from other sources before building the launcher.
/// These come over IPC / from sibling config at runtime; the loader takes them
/// as plain data so it stays testable.
#[derive(Debug, Default, Clone)]
pub struct ProviderInputs {
    /// Recent projects `(name, root)` from the cockpit's cache (over IPC).
    pub recent_projects: Vec<(String, PathBuf)>,
    /// Available theme names.
    pub themes: Vec<String>,
    /// Org capture templates `(key, title)` from `org.toml`.
    pub org_templates: Vec<(String, String)>,
    /// Lua launcher actions harvested from loaded extensions.
    pub lua_actions: Vec<LuaAction>,
}

/// Assemble a [`Launcher`] from `config`, the discovered mise tasks, and the
/// runtime `inputs`. `home` expands a leading `~` in configured project paths;
/// `fs` parses each project's `mise.toml`.
pub fn build_launcher(
    config: &LauncherConfig,
    inputs: &ProviderInputs,
    home: Option<&Path>,
    fs: &dyn FileSystem,
) -> Launcher {
    let mut launcher = Launcher::new().with_max_rows(config.launcher.ui.max_rows.max(1));

    if config.providers.builtins {
        launcher
            .register(Box::new(CalculatorProvider))
            .register(Box::new(UrlProvider));
        if !inputs.recent_projects.is_empty() {
            launcher.register(Box::new(RecentProjectsProvider::new(
                inputs.recent_projects.clone(),
            )));
        }
        if !inputs.themes.is_empty() {
            launcher.register(Box::new(ThemeProvider::new(inputs.themes.clone())));
        }
        if !inputs.org_templates.is_empty() {
            launcher.register(Box::new(OrgProvider::new(inputs.org_templates.clone())));
        }
    }

    if config.providers.mise {
        let roots = expand_paths(&config.mise.projects.paths, home);
        launcher.register(Box::new(MiseTasksProvider::from_projects(fs, roots)));
    }

    if config.providers.lua && !inputs.lua_actions.is_empty() {
        launcher.register(Box::new(LuaActionsProvider::new(
            inputs.lua_actions.clone(),
        )));
    }

    launcher
}

/// Expand a leading `~` / `~/` in each configured path against `home`.
/// Paths without a tilde pass through unchanged; if `home` is unknown a bare
/// `~` path is left as-is (the mise provider will simply find no project).
pub fn expand_paths(paths: &[String], home: Option<&Path>) -> Vec<PathBuf> {
    paths.iter().map(|raw| expand_tilde(raw, home)).collect()
}

fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if raw == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

/// Human label for the configured popover position (for the CLI / status).
pub fn position_label(position: LauncherPosition) -> &'static str {
    match position {
        LauncherPosition::Centred => "centred",
        LauncherPosition::Top => "top",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::env::FakeFileSystem;

    #[test]
    fn expands_tilde_against_home() {
        let home = PathBuf::from("/home/jane");
        let paths = expand_paths(
            &[
                "~/code/work".to_string(),
                "~".to_string(),
                "/abs/path".to_string(),
            ],
            Some(&home),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/jane/code/work"),
                PathBuf::from("/home/jane"),
                PathBuf::from("/abs/path"),
            ]
        );
    }

    #[test]
    fn build_respects_provider_toggles() {
        let mut config = LauncherConfig::default();
        config.providers.lua = false;
        config.providers.mise = false;
        let fs = FakeFileSystem::new();
        let launcher = build_launcher(&config, &ProviderInputs::default(), None, &fs);
        // builtins on (calculator + url), mise/lua off, no runtime inputs.
        let ids: Vec<&str> = launcher.provider_ids().collect();
        assert_eq!(ids, vec!["calculator", "url"]);
    }

    #[test]
    fn build_includes_mise_and_runtime_providers() {
        let fs = FakeFileSystem::new();
        fs.insert_file(
            "/code/app/mise.toml",
            "[tasks.test]\nrun = \"cargo test\"\n",
        );

        let mut config = LauncherConfig::default();
        config.mise.projects.paths = vec!["/code/app".to_string()];

        let inputs = ProviderInputs {
            recent_projects: vec![("app".to_string(), PathBuf::from("/code/app"))],
            themes: vec!["mocha".to_string()],
            org_templates: vec![("t".to_string(), "Todo".to_string())],
            lua_actions: vec![LuaAction::new("ext", "user.x", "Do X")],
        };

        let launcher = build_launcher(&config, &inputs, None, &fs);
        let ids: Vec<&str> = launcher.provider_ids().collect();
        assert_eq!(
            ids,
            vec![
                "calculator",
                "url",
                "projects",
                "theme",
                "org",
                "mise",
                "lua"
            ]
        );

        // A mise task is reachable.
        let hits = launcher.search("test");
        assert!(hits.iter().any(|r| r.provider == "mise"));
    }
}
