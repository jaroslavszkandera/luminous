use log::{error, info};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    SharedLib,
    Daemon,
}

impl Default for BackendKind {
    fn default() -> Self {
        Self::SharedLib
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginCapability {
    Decoder,
    Encoder,
    Interactive,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub backend: BackendKind,
    pub extensions: Vec<String>,
    pub capabilities: Vec<PluginCapability>,
    pub daemon_port: Option<u16>,
    pub interpreter: Option<String>,
    #[serde(default = "default_entry")]
    pub entry: String,
}

fn default_entry() -> String {
    "main.py".to_string()
}

impl PluginManifest {
    pub fn has_capability(&self, cap: &PluginCapability) -> bool {
        self.capabilities.contains(cap)
    }
}

pub fn load_manifest(path: &Path) -> Option<PluginManifest> {
    info!("Loading manifest: {:?}", path);
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read manifest {:?}: {}", path, e);
            return None;
        }
    };
    match serde_json::from_str::<PluginManifest>(&content) {
        Ok(m) => {
            info!("Loaded plugin '{}' v{}", m.name, m.version);
            Some(m)
        }
        Err(e) => {
            error!("Invalid manifest {:?}: {}", path, e);
            None
        }
    }
}
