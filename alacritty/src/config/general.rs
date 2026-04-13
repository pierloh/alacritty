//! Miscellaneous configuration options.

use std::path::PathBuf;

use serde::Serialize;

use alacritty_config_derive::ConfigDeserialize;

use crate::config::ui_config::CustomShaderPaths;

/// General config section.
///
/// This section is for fields which can not be easily categorized,
/// to avoid common TOML issues with root-level fields.
#[derive(ConfigDeserialize, Serialize, Clone, PartialEq, Debug)]
pub struct General {
    /// Configuration file imports.
    ///
    /// This is never read since the field is directly accessed through the config's
    /// [`toml::Value`], but still present to prevent unused field warnings.
    pub import: Vec<String>,

    /// Shell startup directory.
    pub working_directory: Option<PathBuf>,

    /// Live config reload.
    pub live_config_reload: bool,

    /// Offer IPC through a unix socket.
    #[allow(unused)]
    pub ipc_socket: bool,

    /// Custom post-process shaders (chained in order).
    #[config(alias = "custom-shader")]
    pub custom_shader: CustomShaderPaths,
}

impl Default for General {
    fn default() -> Self {
        Self {
            live_config_reload: true,
            ipc_socket: true,
            working_directory: Default::default(),
            import: Default::default(),
            custom_shader: Default::default(),
        }
    }
}
