use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::{Child, Command};

include!(concat!(env!("OUT_DIR"), "/embedded_mihomo.rs"));

#[derive(Debug)]
pub struct MihomoProcess {
    binary_path: Option<PathBuf>,
    config_path: PathBuf,
    process: Option<Child>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MihomoStatus {
    pub running: bool,
    pub binary_path: Option<String>,
    pub config_path: String,
}

impl MihomoProcess {
    pub fn new() -> Self {
        let data_dir = default_data_dir();
        let binary_path = std::env::var("MIHOMO_BINARY")
            .ok()
            .map(PathBuf::from)
            .or_else(|| extract_embedded_mihomo(&data_dir).ok().flatten())
            .or_else(find_mihomo_in_path)
            .or_else(find_mihomo_next_to_exe);

        Self::with_paths(binary_path, data_dir.join("mihomo.yaml"))
    }

    pub fn with_binary(path: impl Into<PathBuf>) -> Self {
        Self::with_paths(Some(path.into()), default_data_dir().join("mihomo.yaml"))
    }

    fn with_paths(binary_path: Option<PathBuf>, config_path: PathBuf) -> Self {
        Self {
            binary_path,
            config_path,
            process: None,
        }
    }

    pub fn is_available(&self) -> bool {
        self.binary_path.as_ref().is_some_and(|path| path.exists())
    }

    pub fn status(&mut self) -> MihomoStatus {
        MihomoStatus {
            running: self.is_running(),
            binary_path: self
                .binary_path
                .as_ref()
                .map(|path| path.display().to_string()),
            config_path: self.config_path.display().to_string(),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn is_running(&mut self) -> bool {
        let Some(child) = self.process.as_mut() else {
            return false;
        };

        match child.try_wait() {
            Ok(Some(_status)) => {
                self.process = None;
                false
            }
            Ok(None) => true,
            Err(_) => {
                self.process = None;
                false
            }
        }
    }

    pub async fn start(&mut self, config_yaml: &str) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_running() {
            return Ok(());
        }

        let binary = self
            .binary_path
            .as_ref()
            .filter(|path| path.exists())
            .ok_or("mihomo binary not found; set MIHOMO_BINARY, build with MIHOMO_EMBED_PATH, or place mihomo in PATH")?;

        write_config_atomically(&self.config_path, config_yaml)?;

        let child = Command::new(binary)
            .arg("-f")
            .arg(&self.config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        self.process = Some(child);
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut child) = self.process.take() {
            child.kill().await.ok();
            child.wait().await.ok();
        }
        Ok(())
    }
}

impl Default for MihomoProcess {
    fn default() -> Self {
        Self::new()
    }
}

fn find_mihomo_in_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("mihomo");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn find_mihomo_next_to_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("mihomo");
    candidate.exists().then_some(candidate)
}

fn extract_embedded_mihomo(data_dir: &Path) -> io::Result<Option<PathBuf>> {
    let Some(bytes) = EMBEDDED_MIHOMO else {
        return Ok(None);
    };

    fs::create_dir_all(data_dir)?;
    let path = data_dir.join("mihomo-embedded");
    let should_write = fs::read(&path).map_or(true, |existing| existing != bytes);
    if should_write {
        fs::write(&path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions)?;
        }
    }

    Ok(Some(path))
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RUST_PROXY_MANAGER_DATA_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from("data")
}

fn write_config_atomically(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("yaml.tmp");
    fs::write(&tmp, content)?;
    fs::rename(tmp, path)?;
    Ok(())
}
