//! `~/.config/gravevault/config.toml`
//!
//! ```toml
//! [git_mirror]
//! github_owner = "cybercoretech"
//! gitlab_group = "cybercoretech-backup"
//!
//! [git_mirror.lfs_sources]
//! NeonCoffinAnimations = "~/Animations/NCV_Animations"
//!
//! [warnflare]
//! url = "http://127.0.0.1:8769/v1/events"
//! token_file = "~/.config/nightwatch/warnflare-token"
//! ```

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub git_mirror: Option<GitMirror>,
    pub warnflare: Option<Warnflare>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitMirror {
    pub github_owner: String,
    pub gitlab_group: String,
    /// Local checkouts holding every LFS object, by GitHub repo name. LFS is
    /// pushed from these, never fetched from GitHub (bandwidth quota).
    #[serde(default)]
    pub lfs_sources: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Warnflare {
    pub url: String,
    pub token_file: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

pub fn default_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("gravevault/config.toml")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("gravevault")
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("gravevault")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_example_parses() {
        let doc = include_str!("config.rs");
        let toml: String = doc
            .lines()
            .skip_while(|l| !l.starts_with("//! ```toml"))
            .skip(1)
            .take_while(|l| !l.starts_with("//! ```"))
            .map(|l| l.trim_start_matches("//!").trim_start_matches(' '))
            .collect::<Vec<_>>()
            .join("\n");
        let c: Config = toml::from_str(&toml).unwrap();
        let m = c.git_mirror.unwrap();
        assert_eq!(m.gitlab_group, "cybercoretech-backup");
        assert_eq!(
            m.lfs_sources["NeonCoffinAnimations"],
            "~/Animations/NCV_Animations"
        );
        assert!(c.warnflare.unwrap().url.ends_with("/v1/events"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>(
            "[git_mirror]\ngithub_owner='a'\ngitlab_group='b'\nforce=true"
        )
        .is_err());
    }
}
