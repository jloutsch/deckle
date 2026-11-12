// SPDX-License-Identifier: GPL-3.0-or-later
//! Signing in to a reMarkable over SSH (fixed-layout spec §12.5): the password from the
//! keychain, the host key trusted on first use and then pinned beside `digest.toml`. Shared by
//! the command line and the app's Devices tab.

use std::path::{Path, PathBuf};

/// The reMarkable's pinned SSH host key: `remarkable-hostkey.json` beside `digest.toml`.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct HostKeyPin {
    pub host: String,
    pub fingerprint: String,
}

pub fn hostkey_path(config: &Path) -> PathBuf {
    config
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("remarkable-hostkey.json")
}

/// Connects to the reMarkable over SSH as `[remarkable]` says, with the password from the
/// keychain (or `DECKLE_REMARKABLE_PASSWORD` without one) and the host key trusted on first
/// use, then pinned (fixed-layout spec §12.5). The fingerprint comes back with the shell.
pub fn connect_with(
    rm: &crate::config::RemarkableConfig,
    config: &Path,
) -> Result<(deckle_core::remarkable_ssh::SshShell, String), String> {
    use crate::secret::{self, SystemKeyring};
    let host = rm.ssh_host();
    // A pin that is there but does not read is not "no pin": that would trust whatever key
    // answers and write over the pin. Nothing connects until it reads.
    let pin_file = hostkey_path(config);
    let pinned: Option<HostKeyPin> = deckle_core::state_file::read(&pin_file)
        .map_err(|e| format!("{e} It pins the reMarkable's SSH host key."))?
        .filter(|p: &HostKeyPin| p.host == host);
    let (password, _) =
        secret::remarkable_password(host, &SystemKeyring, &|k| std::env::var(k).ok())
            .map_err(|e| e.to_string())?;
    let c = deckle_core::remarkable_ssh::connect(
        host,
        rm.ssh_port(),
        &password,
        pinned.as_ref().map(|p| p.fingerprint.as_str()),
        std::time::Duration::from_secs(5),
    )
    .map_err(|e| e.to_string())?;
    if pinned.is_none() {
        let pin = HostKeyPin {
            host: host.to_owned(),
            fingerprint: c.fingerprint.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&pin).expect("two strings");
        deckle_core::state_file::save_owner::<HostKeyPin>(&pin_file, &bytes, false)
            .map_err(|e| format!("couldn't pin the host key: {e}"))?;
    }
    Ok((c.shell, c.fingerprint))
}

pub fn connect(
    rm: &crate::config::RemarkableConfig,
    config: &Path,
) -> Result<deckle_core::remarkable_ssh::SshShell, String> {
    connect_with(rm, config).map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pin that does not read is not "no pin": nothing connects (which would trust whatever
    /// key answers) and the file is left as it was, named in the error.
    #[test]
    fn a_damaged_host_key_pin_refuses_to_connect() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("digest.toml");
        let pin = hostkey_path(&config);
        let cut = br#"{"host": "192.0.2.1", "fingerprint": "SHA256:made-up"#;
        std::fs::write(&pin, cut).unwrap();
        let rm = crate::config::RemarkableConfig {
            ssh_host: "192.0.2.1".into(),
            ..Default::default()
        };
        let e = connect_with(&rm, &config).err().expect("refused");
        assert!(e.contains(&pin.display().to_string()), "{e}");
        assert_eq!(std::fs::read(&pin).unwrap(), cut);
    }
}
