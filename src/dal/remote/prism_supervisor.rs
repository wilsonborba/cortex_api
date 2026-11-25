//! Owns the one Prism server process and one allowlisted loaded Bonsai artifact.
//! The server is never started at application boot and is not a system service.
use crate::{
    core::config::Config,
    domain::models::{driver::DriverError, model_profile::ModelProfile},
};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

const SERVER_SHA256: &str = "89e7f20b23b97bf77b91a0143a168763f7835fd6e0a7a7a23bb70951d991c02c";

struct OwnedServer {
    profile: ModelProfile,
    child: Child,
}

#[derive(Default)]
pub struct PrismSupervisor {
    child: Mutex<Option<OwnedServer>>,
}

impl PrismSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop only this supervisor's child process group. No URL/API kill request is used.
    pub async fn stop(&self) -> Result<(), DriverError> {
        let mut slot = self.child.lock().await;
        stop_owned(&mut slot).await
    }

    pub async fn is_running(&self, profile: ModelProfile) -> Result<bool, DriverError> {
        let mut slot = self.child.lock().await;
        match slot.as_mut() {
            Some(owned) if owned.profile == profile => Ok(owned
                .child
                .try_wait()
                .map_err(|_| DriverError::Ollama)?
                .is_none()),
            _ => Ok(false),
        }
    }

    /// Hash the pinned runtime and model before starting the fixed server command.
    /// Repeated requests for the same healthy profile keep the one process loaded.
    pub async fn ensure(&self, config: &Config, profile: ModelProfile) -> Result<(), DriverError> {
        if !config.prism_url_is_loopback() {
            return Err(DriverError::InvalidInput);
        }
        let alias = profile.runtime_alias().ok_or(DriverError::InvalidInput)?;
        let filename = profile
            .artifact_filename()
            .ok_or(DriverError::InvalidInput)?;
        let expected_model_hash = profile.artifact_sha256().ok_or(DriverError::InvalidInput)?;
        let mut slot = self.child.lock().await;
        if let Some(owned) = slot.as_mut() {
            if owned.profile == profile
                && owned
                    .child
                    .try_wait()
                    .map_err(|_| DriverError::Ollama)?
                    .is_none()
            {
                return Ok(());
            }
        }
        stop_owned(&mut slot).await?;
        let runtime = config.prism_server_path.clone();
        let model = config.bonsai_model_directory.join(filename);
        let runtime_hash = tokio::time::timeout(
            Duration::from_secs(90),
            tokio::task::spawn_blocking({
                let runtime = runtime.clone();
                move || sha256(&runtime)
            }),
        )
        .await
        .map_err(|_| DriverError::Ollama)?
        .map_err(|_| DriverError::Ollama)??;
        let model_hash = tokio::time::timeout(
            Duration::from_secs(180),
            tokio::task::spawn_blocking({
                let model = model.clone();
                move || sha256(&model)
            }),
        )
        .await
        .map_err(|_| DriverError::Ollama)?
        .map_err(|_| DriverError::Ollama)??;
        if runtime_hash != SERVER_SHA256 || model_hash != expected_model_hash {
            return Err(DriverError::Ollama);
        }
        let endpoint = url::Url::parse(&config.prism_url).map_err(|_| DriverError::InvalidInput)?;
        let port = endpoint
            .port_or_known_default()
            .ok_or(DriverError::InvalidInput)?;
        let mut command = Command::new(runtime);
        let runtime_dir = config
            .prism_server_path
            .parent()
            .ok_or(DriverError::InvalidInput)?;
        let mut library_path = std::ffi::OsString::from(runtime_dir);
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            library_path.push(":");
            library_path.push(existing);
        }
        command.env("LD_LIBRARY_PATH", library_path);
        command
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--model",
                model.to_str().ok_or(DriverError::InvalidInput)?,
                "--alias",
                alias,
                "--ctx-size",
                "8192",
                "--parallel",
                "1",
                "--batch-size",
                "1024",
                "--ubatch-size",
                "256",
                "--threads",
                "6",
                "--threads-batch",
                "6",
                "--gpu-layers",
                "all",
                "--flash-attn",
                "on",
                "--cache-type-k",
                "q8_0",
                "--cache-type-v",
                "q8_0",
                "--reasoning-format",
                "deepseek",
                "--min-p",
                "0",
                "--jinja",
                "--no-webui",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if profile.uses_mtp() {
            command.args([
                "--spec-type",
                "draft-mtp",
                "--spec-draft-n-min",
                "0",
                "--spec-draft-n-max",
                "3",
                "--spec-draft-p-min",
                "0",
            ]);
        }
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(|_| DriverError::Ollama)?;
        *slot = Some(OwnedServer { profile, child });
        Ok(())
    }
}

fn sha256(path: &PathBuf) -> Result<String, DriverError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|_| DriverError::Ollama)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| DriverError::Ollama)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn stop_owned(slot: &mut Option<OwnedServer>) -> Result<(), DriverError> {
    let Some(mut owned) = slot.take() else {
        return Ok(());
    };
    let pid = owned.child.id();
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: the child was started in a new process group owned by this instance.
        let _ = unsafe { libc::kill(-(pid as i32), libc::SIGTERM) };
    }
    let result = tokio::time::timeout(Duration::from_secs(5), owned.child.wait()).await;
    if !matches!(result, Ok(Ok(_))) {
        #[cfg(unix)]
        if let Some(pid) = pid {
            // SAFETY: same owned process group as above; only our process group is signalled.
            let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        let _ = owned.child.start_kill();
        tokio::time::timeout(Duration::from_secs(5), owned.child.wait())
            .await
            .map_err(|_| DriverError::Ollama)?
            .map_err(|_| DriverError::Ollama)?;
    }
    #[cfg(unix)]
    if let Some(pid) = pid {
        let group_exited = tokio::time::timeout(Duration::from_secs(5), async {
            while process_group_exists(pid) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .is_ok();
        if !group_exited {
            // SAFETY: pid is the process-group ID assigned to this owned child.
            let _ = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            let killed = tokio::time::timeout(Duration::from_secs(2), async {
                while process_group_exists(pid) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .is_ok();
            if !killed {
                return Err(DriverError::Ollama);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn process_group_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks the existence of the supervisor-owned group.
    if unsafe { libc::kill(-(pid as i32), 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_reaps_only_the_owned_process_group() {
        let mut command = Command::new("/usr/bin/sleep");
        command
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().expect("test child");
        let supervisor = PrismSupervisor {
            child: Mutex::new(Some(OwnedServer {
                profile: ModelProfile::Bonsai25Official,
                child,
            })),
        };
        tokio::time::timeout(Duration::from_secs(2), supervisor.stop())
            .await
            .expect("bounded stop")
            .unwrap();
        assert!(supervisor.child.lock().await.is_none());
    }

    #[test]
    fn profiles_resolve_only_pinned_artifact_names_and_hashes() {
        for profile in [
            ModelProfile::Bonsai25Official,
            ModelProfile::Bonsai25Uncensored,
        ] {
            assert!(profile.artifact_filename().unwrap().ends_with(".gguf"));
            assert_eq!(profile.artifact_sha256().unwrap().len(), 64);
            assert!(profile.runtime_alias().unwrap().starts_with("bonsai25-"));
        }
        assert!(ModelProfile::CortexT0.artifact_filename().is_none());
    }
}
