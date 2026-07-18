use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::thread;
use std::time::{Duration, Instant};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const PIN_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../config/fastvep-pin.json"
));

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FastVepPin {
    upstream_version: String,
    #[serde(rename = "windowsX86_64")]
    windows_x86_64: WindowsPin,
}

#[derive(Deserialize)]
struct WindowsPin {
    sha256: String,
}

static PIN: LazyLock<FastVepPin> = LazyLock::new(|| {
    serde_json::from_str(PIN_MANIFEST).expect("config/fastvep-pin.json must be valid")
});

fn pinned_sha256() -> &'static str {
    PIN.windows_x86_64.sha256.as_str()
}

fn pinned_version() -> String {
    format!("fastvep {}", PIN.upstream_version)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub ready: bool,
    pub state: &'static str,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub sha256: Option<String>,
    pub expected_sha256: &'static str,
    pub managed: bool,
    pub next_action: &'static str,
}

pub fn readiness() -> Readiness {
    let home = super::portable_home().ok();
    readiness_with(home.as_deref(), std::env::var_os("ANNOCAT_FASTVEP"))
}

fn readiness_with(home: Option<&Path>, configured: Option<std::ffi::OsString>) -> Readiness {
    let expected_sha256 = pinned_sha256();
    let managed = home.map(|root| {
        root.join("tools").join("fastvep").join(if cfg!(windows) {
            "fastvep.exe"
        } else {
            "fastvep"
        })
    });
    let development = std::env::current_dir().ok().map(|root| {
        root.join("tools").join("fastvep").join(if cfg!(windows) {
            "fastvep.exe"
        } else {
            "fastvep"
        })
    });
    let mut candidates = configured
        .map(PathBuf::from)
        .into_iter()
        .chain(managed.clone())
        .collect::<Vec<_>>();
    if let Some(path) = development.as_ref()
        && !candidates.contains(path)
    {
        candidates.push(path.clone());
    }

    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        let is_managed =
            managed.as_ref() == Some(&candidate) || development.as_ref() == Some(&candidate);
        let Ok(sha256) = sha256_file(&candidate) else {
            continue;
        };
        if sha256 != expected_sha256 {
            return Readiness {
                ready: false,
                state: "checksum-mismatch",
                managed: is_managed,
                executable: Some(candidate),
                version: None,
                sha256: Some(sha256),
                expected_sha256,
                next_action: "Repair the fastVEP installation before annotation",
            };
        }
        if let Ok(version) = command_output(&candidate, &["--version"]) {
            let version = first_line(&version);
            if version != pinned_version() {
                return Readiness {
                    ready: false,
                    state: "version-mismatch",
                    managed: is_managed,
                    executable: Some(candidate),
                    version: Some(version),
                    sha256: Some(sha256),
                    expected_sha256,
                    next_action: "Repair the fastVEP installation before annotation",
                };
            }
            return Readiness {
                ready: true,
                state: "ready",
                managed: is_managed,
                executable: Some(candidate),
                version: Some(version),
                sha256: Some(sha256),
                expected_sha256,
                next_action: "Install and verify the pinned fastVEP transcript and fastSA resources",
            };
        }
    }

    if let Ok(version) = command_output(Path::new("fastvep"), &["--version"]) {
        return Readiness {
            ready: true,
            state: "ready",
            executable: Some(PathBuf::from("fastvep")),
            version: Some(first_line(&version)),
            sha256: None,
            expected_sha256,
            managed: false,
            next_action: "Validate this fastVEP build, then install the pinned transcript and fastSA resources",
        };
    }

    Readiness {
        ready: false,
        state: "missing",
        executable: managed,
        version: None,
        sha256: None,
        expected_sha256,
        managed: true,
        next_action: "Install a pinned fastVEP Windows binary into AnnoCat's tools directory",
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn readiness_json() -> String {
    serde_json::to_string(&readiness()).unwrap_or_else(|error| {
        format!(
            "{{\"ready\":false,\"error\":\"{}\"}}",
            super::json_escape(&error.to_string())
        )
    })
}

pub fn supports_sa_verify(executable: &Path) -> bool {
    command_output(executable, &["sa-verify", "--help"]).is_ok()
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or_default().trim().to_string()
}

fn command_output(program: &Path, arguments: &[&str]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("cannot read fastVEP output: {error}"))?;
                if status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    return Ok(if stdout.is_empty() { stderr } else { stdout });
                }
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            Ok(None) if started.elapsed() < COMMAND_TIMEOUT => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("fastVEP did not respond within 5 seconds".into());
            }
            Err(error) => return Err(format!("cannot query fastVEP: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_managed_binary_reports_its_expected_location() {
        let home = Path::new("C:/portable/annocat");
        let report = readiness_with(Some(home), None);
        assert!(!report.ready);
        assert!(report.managed);
        assert!(report.executable.unwrap().starts_with(home));
    }

    #[test]
    fn readiness_report_is_machine_readable() {
        let report = Readiness {
            ready: false,
            state: "missing",
            executable: Some(PathBuf::from("tools/fastvep/fastvep.exe")),
            version: None,
            sha256: None,
            expected_sha256: pinned_sha256(),
            managed: true,
            next_action: "Install fastVEP",
        };
        let value = serde_json::to_value(report).unwrap();
        assert!(value["ready"].is_boolean());
        assert!(value["nextAction"].is_string());
    }
}
