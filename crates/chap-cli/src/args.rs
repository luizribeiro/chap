use crate::commands::Command;
use clap::Parser;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(version, about = "A plugin-powered coding agent")]
pub(crate) struct Cli {
    /// Configuration file to read. Resolution order: --config, CHAP_CONFIG,
    /// ./chap.json if present, then $XDG_CONFIG_HOME/chap/chap.json (or
    /// ~/.config/chap/chap.json).
    #[arg(long, env = "CHAP_CONFIG", global = true)]
    pub(crate) config: Option<PathBuf>,

    /// Directory mounted at /mnt/workspace when agent.workspace is configured.
    #[arg(long, env = "CHAP_WORKSPACE", global = true, value_name = "DIR")]
    pub(crate) workspace: Option<PathBuf>,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

pub(crate) fn resolve_workspace_directory(
    configured: Option<PathBuf>,
    current_directory: &Path,
) -> Result<PathBuf, String> {
    let path = configured.unwrap_or_else(|| current_directory.to_path_buf());
    let resolved = path.canonicalize().map_err(|source| {
        format!(
            "failed to resolve workspace directory `{}`: {source}",
            path.display()
        )
    })?;
    if !resolved.is_dir() {
        return Err(format!(
            "workspace path `{}` is not a directory",
            resolved.display()
        ));
    }
    Ok(resolved)
}

pub(crate) fn resolve_config_path(
    configured: Option<PathBuf>,
    local: &Path,
    xdg_config_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        return Ok(path);
    }
    if config_exists(local)? {
        return Ok(local.to_path_buf());
    }

    let config_root = if let Some(path) = xdg_config_home.filter(|path| !path.is_empty()) {
        PathBuf::from(path)
    } else if let Some(path) = home.filter(|path| !path.is_empty()) {
        PathBuf::from(path).join(".config")
    } else {
        return Err(format!(
            "no config found: tried {}; cannot determine the personal config path because neither XDG_CONFIG_HOME nor HOME is set to a non-empty value (set --config or CHAP_CONFIG)",
            local.display()
        ));
    };
    let personal = config_root.join("chap/chap.json");
    if config_exists(&personal)? {
        return Ok(personal);
    }

    Err(format!(
        "no config found: tried {} and {} (set --config or CHAP_CONFIG)",
        local.display(),
        personal.display()
    ))
}

fn config_exists(path: &Path) -> Result<bool, String> {
    path.try_exists()
        .map_err(|source| format!("failed to check config path `{}`: {source}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chap_core::{AgentBuilder, LoadError};
    use clap::CommandFactory;

    #[test]
    fn starts_the_tui_when_no_subcommand_is_given() {
        let cli = Cli::try_parse_from(["chap"]).unwrap();

        assert!(cli.command.is_none());
    }

    #[test]
    fn config_flag_is_paired_with_the_environment_without_a_static_default() {
        let command = Cli::command();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "config")
            .unwrap();

        assert_eq!(argument.get_env(), Some(OsStr::new("CHAP_CONFIG")));
        assert!(argument.get_default_values().is_empty());

        let cli = Cli::try_parse_from(["chap", "--config", "explicit.json"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("explicit.json")));
    }

    #[test]
    fn workspace_flag_is_paired_with_the_environment_without_a_static_default() {
        let command = Cli::command();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "workspace")
            .unwrap();

        assert_eq!(argument.get_env(), Some(OsStr::new("CHAP_WORKSPACE")));
        assert!(argument.get_default_values().is_empty());

        let cli = Cli::try_parse_from(["chap", "--workspace", "project"]).unwrap();
        assert_eq!(cli.workspace, Some(PathBuf::from("project")));
    }

    #[test]
    fn explicit_workspace_directory_is_canonicalized() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();

        let resolved =
            resolve_workspace_directory(Some(workspace.clone()), Path::new("/unused")).unwrap();

        assert_eq!(resolved, workspace.canonicalize().unwrap());
    }

    #[test]
    fn workspace_directory_defaults_to_the_current_directory() {
        let directory = tempfile::tempdir().unwrap();

        let resolved = resolve_workspace_directory(None, directory.path()).unwrap();

        assert_eq!(resolved, directory.path().canonicalize().unwrap());
    }

    #[test]
    fn missing_workspace_directory_error_names_the_path() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-workspace");

        let error =
            resolve_workspace_directory(Some(missing.clone()), Path::new("/unused")).unwrap_err();

        assert!(error.contains(&missing.display().to_string()));
    }

    #[test]
    fn config_flag_wins_over_default_files() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("chap.json");
        std::fs::write(&local, "{}").unwrap();
        let explicit = directory.path().join("explicit.json");

        let resolved = resolve_config_path(
            Some(explicit.clone()),
            &local,
            Some(OsStr::new("/unused/config")),
            Some(OsStr::new("/unused/home")),
        )
        .unwrap();

        assert_eq!(resolved, explicit);
    }

    #[test]
    fn config_from_the_environment_wins_over_default_files() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("chap.json");
        std::fs::write(&local, "{}").unwrap();
        let configured = directory.path().join("from-environment.json");

        let resolved = resolve_config_path(
            Some(configured.clone()),
            &local,
            Some(OsStr::new("/unused/config")),
            Some(OsStr::new("/unused/home")),
        )
        .unwrap();

        assert_eq!(resolved, configured);
    }

    #[test]
    fn local_config_is_the_first_file_default() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("chap.json");
        std::fs::write(&local, "{}").unwrap();

        let resolved = resolve_config_path(
            None,
            &local,
            Some(OsStr::new("/unused/config")),
            Some(OsStr::new("/unused/home")),
        )
        .unwrap();

        assert_eq!(resolved, local);
    }

    #[test]
    fn personal_config_is_used_when_the_local_config_is_absent() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("project/chap.json");
        let config_home = directory.path().join("config");
        let personal = config_home.join("chap/chap.json");
        std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
        std::fs::write(&personal, "{}").unwrap();

        let resolved =
            resolve_config_path(None, &local, Some(config_home.as_os_str()), None).unwrap();

        assert_eq!(resolved, personal);
    }

    #[test]
    fn invalid_local_config_fails_without_falling_through_to_personal_config() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("chap.json");
        std::fs::write(&local, "{").unwrap();
        let config_home = directory.path().join("config");
        let personal = config_home.join("chap/chap.json");
        std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
        std::fs::write(&personal, "{}").unwrap();

        let resolved =
            resolve_config_path(None, &local, Some(config_home.as_os_str()), None).unwrap();
        let Err(error) = AgentBuilder::load(&resolved) else {
            panic!("invalid local config unexpectedly loaded");
        };

        assert_eq!(resolved, local);
        assert!(matches!(error, LoadError::ParseConfig { path, .. } if path == local));
    }

    #[test]
    fn missing_config_error_names_every_probed_path() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("project/chap.json");
        let home = directory.path().join("home");
        let personal = home.join(".config/chap/chap.json");

        let error = resolve_config_path(None, &local, None, Some(home.as_os_str())).unwrap_err();

        assert_eq!(
            error,
            format!(
                "no config found: tried {} and {} (set --config or CHAP_CONFIG)",
                local.display(),
                personal.display()
            )
        );
    }

    #[test]
    fn xdg_config_home_is_respected() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("project/chap.json");
        let config_home = directory.path().join("xdg");
        let personal = config_home.join("chap/chap.json");
        std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
        std::fs::write(&personal, "{}").unwrap();

        let resolved = resolve_config_path(
            None,
            &local,
            Some(config_home.as_os_str()),
            Some(OsStr::new("/unused/home")),
        )
        .unwrap();

        assert_eq!(resolved, personal);
    }

    #[test]
    fn empty_xdg_config_home_falls_back_to_home() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("project/chap.json");
        let home = directory.path().join("home");
        let personal = home.join(".config/chap/chap.json");
        std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
        std::fs::write(&personal, "{}").unwrap();

        let resolved =
            resolve_config_path(None, &local, Some(OsStr::new("")), Some(home.as_os_str()))
                .unwrap();

        assert_eq!(resolved, personal);
    }
}
