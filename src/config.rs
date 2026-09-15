//! On-disk configuration for ferum-greet, loaded from `/etc/ferum-greet/config.toml`
//! (or a path given with `--config`). Every field has a sane default so a
//! missing file, or a file missing some fields, still produces a usable config.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The wallpaper baked in at build time by the Nix package (a nature photo
/// fetched from a fixed, hash-pinned URL - see `flake.nix`). Falls back to a
/// path that won't exist outside of the Nix build, in which case the
/// renderer just falls back to a solid color background.
pub const BUILTIN_DEFAULT_WALLPAPER: &str =
    match option_env!("FERUM_GREET_DEFAULT_WALLPAPER") {
        Some(p) => p,
        None => "/etc/ferum-greet/default-wallpaper.jpg",
    };

pub const DEFAULT_CONFIG_PATH: &str = "/etc/ferum-greet/config.toml";
pub const DEFAULT_STATE_DIR: &str = "/var/lib/ferum-greet";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// DRM device node to render to, e.g. `/dev/dri/card0`. `auto` picks the
    /// first device with a connected connector.
    pub drm_device: String,

    /// Background configuration.
    pub background: BackgroundConfig,

    /// Whether to remember and pre-fill the username of the last successful
    /// login.
    pub remember_last_user: bool,

    /// Directory used to persist small bits of state (currently just the
    /// last logged-in username), when `remember_last_user` is enabled.
    pub state_dir: PathBuf,

    /// The list of desktop environments / sessions offered to the user.
    pub sessions: Vec<SessionConfig>,

    /// Name of the session (must match a `sessions[].name`) to preselect.
    /// If unset, the first configured session is used.
    pub default_session: Option<String>,

    /// UI tuning.
    pub theme: ThemeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            drm_device: "auto".to_string(),
            background: BackgroundConfig::default(),
            remember_last_user: true,
            state_dir: PathBuf::from(DEFAULT_STATE_DIR),
            sessions: vec![SessionConfig {
                name: "Default shell".to_string(),
                exec: "/bin/sh -l".to_string(),
                env: Vec::new(),
            }],
            default_session: None,
            theme: ThemeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "kind")]
pub enum BackgroundConfig {
    /// The wallpaper bundled by the Nix package.
    Default,
    /// A solid RGB color, each channel 0-255.
    Color { r: u8, g: u8, b: u8 },
    /// A raster (PNG/JPEG) or SVG image on disk.
    Image { path: PathBuf },
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        BackgroundConfig::Default
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionConfig {
    // (kept `Clone` so `App` can hold on to the selected entry independently
    // of the config it came from)
    /// Human-readable name shown in the session picker.
    pub name: String,
    /// Command line executed by greetd to start the session, e.g.
    /// `sway` or `gnome-session` or `startplasma-wayland`.
    pub exec: String,
    /// Extra `"KEY=VALUE"` environment variables greetd sets for the
    /// session process, e.g. `["RUST_LOG=debug"]` for a debug variant of an
    /// existing session.
    #[serde(default)]
    pub env: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub accent_color: [u8; 3],
    pub font_family: String,
    pub font_size: f32,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            accent_color: [90, 150, 240],
            font_family: "sans-serif".to_string(),
            font_size: 20.0,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => cfg,
                Err(err) => {
                    log::error!("failed to parse config at {}: {err}", path.display());
                    Config::default()
                }
            },
            Err(err) => {
                log::info!(
                    "no config at {} ({err}), using built-in defaults",
                    path.display()
                );
                Config::default()
            }
        }
    }

    pub fn last_user_file(&self) -> PathBuf {
        self.state_dir.join("last_user")
    }

    pub fn selected_session_index(&self) -> usize {
        match &self.default_session {
            Some(name) => self
                .sessions
                .iter()
                .position(|s| &s.name == name)
                .unwrap_or(0),
            None => 0,
        }
    }
}
