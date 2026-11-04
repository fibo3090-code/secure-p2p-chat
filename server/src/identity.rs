//! Persistent server identity (Phase 1, slice 3).
//!
//! The server's RSA identity is stored as a PEM file under the operator's data
//! dir and reused across restarts, so its fingerprint — which clients pin via TOFU
//! on first connect — stays constant. This mirrors how an SSH host key works: the
//! private key lives on the server host (owner-only file permissions) rather than
//! being password-encrypted like the client's interactive identity.

use std::path::Path;

use anyhow::{Context, Result};
use messenger_core::core::{generate_rsa_keypair, pem_decode_private, pem_encode_private};
use messenger_core::RSA_KEY_BITS;
use rsa::RsaPrivateKey;

const IDENTITY_FILE: &str = "server_identity.pem";

/// Load the server identity from `<data_dir>/server_identity.pem`, creating and
/// saving a fresh one on first run.
pub fn load_or_create_server_identity(data_dir: &Path) -> Result<RsaPrivateKey> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    let path = data_dir.join(IDENTITY_FILE);

    if path.exists() {
        let pem = std::fs::read_to_string(&path)
            .with_context(|| format!("reading server identity {}", path.display()))?;
        pem_decode_private(&pem).context("decoding server identity")
    } else {
        let key = generate_rsa_keypair(RSA_KEY_BITS)?;
        let pem = pem_encode_private(&key)?;
        write_private_pem(&path, &pem)?;
        Ok(key)
    }
}

/// Write a private-key PEM: owner-only **from creation**, and atomically.
///
/// This used to `fs::write` and then `chmod 0600`. Under a typical 022 umask
/// that left the server's private key world-readable (0644) for the window
/// between the two calls, and a crash mid-write left a truncated PEM that the
/// next start refuses to load. `write_file_atomic` creates the temp file 0600,
/// fsyncs it and renames it into place, so neither state is ever observable.
fn write_private_pem(path: &Path, pem: &str) -> Result<()> {
    messenger_core::util::write_file_atomic(path, pem.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::core::{fingerprint_pubkey, pem_encode_public};
    use rsa::RsaPublicKey;

    fn fingerprint(key: &RsaPrivateKey) -> String {
        fingerprint_pubkey(
            pem_encode_public(&RsaPublicKey::from(key))
                .unwrap()
                .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn identity_is_stable_across_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_server_identity(dir.path()).unwrap();
        let second = load_or_create_server_identity(dir.path()).unwrap();
        assert_eq!(
            fingerprint(&first),
            fingerprint(&second),
            "the server fingerprint must be stable across restarts"
        );
    }

    #[test]
    fn creates_data_dir_and_persists_file() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("does/not/exist/yet");
        load_or_create_server_identity(&nested).unwrap();
        assert!(nested.join(IDENTITY_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_key_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        load_or_create_server_identity(dir.path()).unwrap();
        let meta = std::fs::metadata(dir.path().join(IDENTITY_FILE)).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    /// A damaged key file must stop the server, never be silently replaced:
    /// a new key is a new fingerprint, and every client that pinned the old
    /// one would then be told the server is impersonating itself.
    #[test]
    fn a_damaged_identity_is_an_error_not_a_new_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE);
        // A truncated PEM. Assembled at runtime so a secret scanner reading the
        // source does not mistake this fixture for a committed key.
        let damaged = format!("-----BEGIN {}-----\ntrunc", "PRIVATE KEY");
        std::fs::write(&path, &damaged).unwrap();
        assert!(load_or_create_server_identity(dir.path()).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            damaged.as_bytes(),
            "the damaged file must be left for the operator to inspect"
        );
    }

    #[test]
    fn creating_the_identity_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        load_or_create_server_identity(dir.path()).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![IDENTITY_FILE.to_string()]);
    }
}
