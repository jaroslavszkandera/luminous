use log::{error, info};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum InteractiveCapability {
    Click,
    Select,
    Text,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PluginCapability {
    Decoder,
    Encoder,
    Interactive(Vec<InteractiveCapability>),
    Search,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub backend: BackendKind,
    pub extensions: Vec<String>,
    pub capabilities: Vec<PluginCapability>,
    pub daemon_ip: Option<String>,
    pub daemon_port: Option<u16>,
    pub interpreter: Option<String>,
    pub entry: Option<String>,
}

impl PluginManifest {
    pub fn has_capability(&self, cap: &PluginCapability) -> bool {
        self.capabilities.contains(cap)
    }
}

fn validate_manifest(manifest: PluginManifest) -> Option<PluginManifest> {
    if (manifest.capabilities.contains(&PluginCapability::Decoder)
        || manifest.capabilities.contains(&PluginCapability::Encoder))
        && manifest.extensions.is_empty()
    {
        error!(
            "Manifest requires at least one extension when decoder or encoder capability is present"
        );
        return None;
    }
    if manifest.capabilities.is_empty() {
        error!("Manifest requires at least one capability");
        return None;
    }

    match manifest.backend {
        BackendKind::Daemon => {
            if manifest.daemon_port.is_none() {
                error!("Daemon backend requires daemon_port");
                return None;
            }
            if manifest.interpreter.is_none() {
                error!("Daemon backend requires interpreter");
                return None;
            }
            if manifest.entry.is_none() {
                error!("Daemon backend requires entry point");
                return None;
            }
        }
        BackendKind::SharedLib => {
            if manifest.daemon_port.is_some() {
                error!("SharedLib backend should not have daemon_port");
                return None;
            }
            if manifest.interpreter.is_some() {
                error!("SharedLib backend should not have interpreter");
                return None;
            }
        }
    }
    Some(manifest)
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
        Ok(m) => validate_manifest(m).and_then(|m| {
            info!("Loaded plugin '{}' v{}", m.name, m.version);
            Some(m)
        }),
        Err(e) => {
            error!("Invalid manifest {:?}: {}", path, e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn load_manifest_daemon_missing_port() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::Daemon,
            extensions: vec![],
            capabilities: vec![PluginCapability::Interactive(vec![])],
            daemon_ip: None,
            daemon_port: None,
            interpreter: Some("python".into()),
            entry: Some("main.py".into()),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_daemon_missing_interpreter() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::Daemon,
            extensions: vec![],
            capabilities: vec![PluginCapability::Interactive(vec![])],
            daemon_ip: None,
            daemon_port: Some(8080),
            interpreter: None,
            entry: Some("main.py".into()),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_daemon_missing_entry() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::Daemon,
            extensions: vec![],
            capabilities: vec![PluginCapability::Interactive(vec![])],
            daemon_ip: None,
            daemon_port: Some(8080),
            interpreter: Some("python".into()),
            entry: None,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_sharedlib_with_daemon_port() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::SharedLib,
            extensions: vec!["jpg".to_string()],
            capabilities: vec![PluginCapability::Decoder],
            daemon_ip: None,
            daemon_port: Some(8080),
            interpreter: None,
            entry: None,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_sharedlib_with_interpreter() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::SharedLib,
            extensions: vec!["jpg".to_string()],
            capabilities: vec![PluginCapability::Decoder],
            daemon_ip: None,
            daemon_port: None,
            interpreter: Some("python".into()),
            entry: None,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_missing_name() {
        let temp_dir = TempDir::new().unwrap();
        let json = r#"{"version":"1.0.0","extensions":["jpg"],"capabilities":["decoder"]}"#;
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_missing_version() {
        let temp_dir = TempDir::new().unwrap();
        let json = r#"{"name":"test","extensions":["jpg"],"capabilities":["decoder"]}"#;
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    #[test]
    fn load_manifest_missing_capabilities() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::SharedLib,
            extensions: vec!["jpg".to_string()],
            capabilities: vec![],
            daemon_ip: None,
            daemon_port: None,
            interpreter: None,
            entry: None,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }

    fn write_manifest(json: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("plugin.json");
        fs::write(&p, json).unwrap();
        (dir, p)
    }

    #[test]
    fn load_manifest_valid_sharedlib_decoder() {
        let m = PluginManifest {
            name: "x".into(),
            version: "1.0.0".into(),
            backend: BackendKind::SharedLib,
            extensions: vec!["png".into()],
            capabilities: vec![PluginCapability::Decoder],
            daemon_ip: None,
            daemon_port: None,
            interpreter: None,
            entry: None,
        };
        let (_d, p) = write_manifest(&serde_json::to_string(&m).unwrap());
        let loaded = load_manifest(&p).unwrap();
        assert_eq!(loaded.name, "x");
        assert_eq!(loaded.backend, BackendKind::SharedLib);
    }

    #[test]
    fn load_manifest_valid_daemon() {
        let m = PluginManifest {
            name: "d".into(),
            version: "1.0.0".into(),
            backend: BackendKind::Daemon,
            extensions: vec![],
            capabilities: vec![PluginCapability::Interactive(vec![
                InteractiveCapability::Click,
            ])],
            daemon_ip: None,
            daemon_port: Some(9000),
            interpreter: Some("python".into()),
            entry: Some("main.py".into()),
        };
        let (_d, p) = write_manifest(&serde_json::to_string(&m).unwrap());
        assert!(load_manifest(&p).is_some());
    }

    #[test]
    fn load_manifest_file_missing() {
        assert!(load_manifest(Path::new("/no/such/plugin.json")).is_none());
    }

    #[test]
    fn load_manifest_invalid_json() {
        let (_d, p) = write_manifest("{not json}");
        assert!(load_manifest(&p).is_none());
    }

    #[test]
    fn has_capability_matches_decoder() {
        let m = PluginManifest {
            name: "x".into(),
            version: "1.0.0".into(),
            backend: BackendKind::SharedLib,
            extensions: vec!["png".into()],
            capabilities: vec![PluginCapability::Decoder],
            daemon_ip: None,
            daemon_port: None,
            interpreter: None,
            entry: None,
        };
        assert!(m.has_capability(&PluginCapability::Decoder));
        assert!(!m.has_capability(&PluginCapability::Encoder));
        assert!(!m.has_capability(&PluginCapability::Search));
    }

    #[test]
    fn unknown_capability_deserializes() {
        let json = r#"{"name":"x","version":"1.0.0","extensions":["png"],"capabilities":["weird_thing","decoder"]}"#;
        let (_d, p) = write_manifest(json);
        let m = load_manifest(&p).unwrap();
        assert!(m.capabilities.contains(&PluginCapability::Unknown));
        assert!(m.capabilities.contains(&PluginCapability::Decoder));
    }

    #[test]
    fn backend_kind_default_is_sharedlib() {
        assert_eq!(BackendKind::default(), BackendKind::SharedLib);
    }

    #[test]
    fn load_manifest_missing_extensions() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = PluginManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            backend: BackendKind::SharedLib,
            extensions: vec![],
            capabilities: vec![PluginCapability::Decoder],
            daemon_ip: None,
            daemon_port: None,
            interpreter: None,
            entry: None,
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let manifest_path = temp_dir.path().join("manifest.json");
        fs::write(&manifest_path, json).unwrap();
        assert!(load_manifest(&manifest_path).is_none());
    }
}
