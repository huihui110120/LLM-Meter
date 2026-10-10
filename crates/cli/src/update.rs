#[cfg(unix)]
use std::{env, path::PathBuf, process::Command};

#[cfg(unix)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Updating LLM Meter from crates.io...");
    let status = Command::new("cargo")
        .args(["install", "--locked", "--force", "llm-meter-cli"])
        .status()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    if !status.success() {
        return Err(format!("Cargo update failed with {status}").into());
    }

    let installed = cargo_bin().join("llm-meter");
    if !installed.is_file() {
        return Err(format!(
            "Cargo completed, but the updated CLI was not found at {}",
            installed.display()
        )
        .into());
    }
    let status = Command::new(&installed)
        .arg("_post-update")
        .status()
        .map_err(|error| format!("could not synchronize the updated installation: {error}"))?;
    if !status.success() {
        return Err("the package was updated, but integration synchronization failed".into());
    }

    let version = Command::new(&installed).arg("--version").output().ok();
    let version = version
        .as_ref()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "LLM Meter".into());
    println!("{version} is installed and ready.");
    Ok(())
}

#[cfg(windows)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    const RELEASES_URL: &str = "https://github.com/huihui110120/LLM-Meter/releases/latest";

    println!("LLM Meter does not replace running executables with Cargo on Windows.");
    println!("Download and run the signed current-user installer from:");
    println!("  {RELEASES_URL}");
    println!("The installer preserves application data and reconciles daemon autostart.");
    crate::platform::open_https_url(RELEASES_URL)?;
    Ok(())
}

#[cfg(unix)]
fn cargo_bin() -> PathBuf {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .unwrap_or_else(|| PathBuf::from(".cargo"))
        .join("bin")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn cargo_bin_ends_with_the_binary_directory() {
        assert_eq!(
            cargo_bin().file_name().and_then(|value| value.to_str()),
            Some("bin")
        );
    }
}
