// SPDX-License-Identifier: MPL-2.0
//! The device key: created on first launch, stored encrypted at rest.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::Arc,
};

use iroh::SecretKey;

use crate::{Error, Result};

pub(crate) const FILE_NAME: &str = "identity.key";
const MAGIC: &[u8; 4] = b"NLK1";

/// The method byte of a key sealed with the desktop's Secret Service. 1 was
/// Windows DPAPI and 2 is the Android Keystore.
pub(crate) const SECRET_SERVICE_PROTECTOR: u8 = 3;

const KEYRING_NEEDED: &str =
    "the desktop keyring that holds the device key is locked or not running; unlock it and reopen G7 Link";

/// Protects the device key at rest. Each platform provides one: the Secret
/// Service on Linux, Android Keystore wrapping on Android (injected by the app).
pub trait KeyProtector: Send + Sync {
    /// A stable one-byte identifier written into the key file.
    fn id(&self) -> u8;
    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>>;
    fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>>;
    /// Whether `stored` was saved in the clear before this protector was in
    /// use, and may be taken in and sealed. By default nothing is: a plain
    /// key dropped into the data folder must not become this device's identity.
    fn is_legacy_plain(&self, _stored: &[u8]) -> bool {
        false
    }
}

/// No protection. Only used where no platform protector exists (tests,
/// headless tools); a warning is logged when it is used for a real key.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainKeyProtector;

impl KeyProtector for PlainKeyProtector {
    fn id(&self) -> u8 {
        0
    }
    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>> {
        Ok(plaintext.to_vec())
    }
    fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>> {
        Ok(ciphertext.to_vec())
    }
}

/// The best protector available on this platform. On Linux this asks the
/// Secret Service and can wait for the user to unlock it, so call it off the
/// UI thread.
pub fn default_protector() -> Arc<dyn KeyProtector> {
    #[cfg(target_os = "linux")]
    {
        crate::keyring::default_protector()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Arc::new(PlainKeyProtector)
    }
}

/// Opens data a store saved. Returns the plain bytes and whether they were
/// saved before sealing was in use, so the store can save them again sealed.
pub fn open_stored(protector: &dyn KeyProtector, stored: &[u8]) -> io::Result<(Vec<u8>, bool)> {
    if protector.is_legacy_plain(stored) {
        return Ok((stored.to_vec(), true));
    }
    Ok((protector.unprotect(stored)?, false))
}

/// Loads the device key from `dir`, creating and saving a new one on first
/// launch.
pub(crate) fn load_or_create(dir: &Path, protector: &dyn KeyProtector) -> Result<SecretKey> {
    let path = dir.join(FILE_NAME);
    match fs::read(&path) {
        Ok(bytes) => load_existing(&path, &bytes, protector),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            if protector.id() == 0 {
                tracing::warn!("device key is stored without platform protection");
            }
            write_atomically(&path, &encode(&key, protector)?)?;
            tracing::info!(device = %crate::device_id(&key.public()).short(), "created device identity");
            Ok(key)
        }
        Err(e) => Err(e.into()),
    }
}

/// The protector for everything else on disk. Data is sealed only once the
/// device key is: a run that can't reach the keyring then stops at the key
/// instead of reading sealed files as if they were plain.
pub(crate) fn protector_for_data(dir: &Path, protector: Arc<dyn KeyProtector>) -> Arc<dyn KeyProtector> {
    let stored = fs::read(dir.join(FILE_NAME)).unwrap_or_default();
    match method_and_payload(&stored) {
        Some((method, _)) if method == protector.id() => protector,
        _ => Arc::new(PlainKeyProtector),
    }
}

fn load_existing(path: &Path, bytes: &[u8], protector: &dyn KeyProtector) -> Result<SecretKey> {
    if let Some((0, payload)) = method_and_payload(bytes)
        && protector.is_legacy_plain(payload)
    {
        let key = decode(bytes, &PlainKeyProtector)?;
        seal_plain_key(path, &key, protector);
        return Ok(key);
    }
    decode(bytes, protector)
}

/// Moves a plain key under `protector`. If that fails the plain file stays,
/// and the key works as it did; the next start tries again.
fn seal_plain_key(path: &Path, key: &SecretKey, protector: &dyn KeyProtector) {
    if !private_to_owner(path) {
        tracing::warn!("device key file isn't private to its owner, so it was not sealed");
        return;
    }
    match replace_with_sealed(path, key, protector) {
        Ok(()) => tracing::info!("device key sealed with the desktop keyring"),
        Err(e) => tracing::warn!(error = %e, "can't seal the device key; it stays as it was"),
    }
}

fn replace_with_sealed(path: &Path, key: &SecretKey, protector: &dyn KeyProtector) -> Result<()> {
    let sealed = encode(key, protector)?;
    if decode(&sealed, protector)?.to_bytes() != key.to_bytes() {
        return Err(Error::Storage("the sealed device key didn't open to the same key".into()));
    }
    write_atomically(path, &sealed)?;
    Ok(())
}

/// A regular file with its folder's owner and no access for anyone else, as
/// this app writes it. Anything else may have been put there by someone else.
fn private_to_owner(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(file) = fs::symlink_metadata(path) else { return false };
    let Some(owner) = path.parent().and_then(|dir| fs::metadata(dir).ok()).map(|dir| dir.uid()) else {
        return false;
    };
    file.is_file() && file.uid() == owner && file.mode() & 0o077 == 0
}

fn method_and_payload(bytes: &[u8]) -> Option<(u8, &[u8])> {
    let (&method, payload) = bytes.strip_prefix(MAGIC.as_slice())?.split_first()?;
    Some((method, payload))
}

fn encode(key: &SecretKey, protector: &dyn KeyProtector) -> Result<Vec<u8>> {
    let sealed = protector.protect(&key.to_bytes())?;
    let mut out = Vec::with_capacity(MAGIC.len() + 1 + sealed.len());
    out.extend_from_slice(MAGIC);
    out.push(protector.id());
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn decode(bytes: &[u8], protector: &dyn KeyProtector) -> Result<SecretKey> {
    let corrupt = || Error::Storage("the device key file is corrupt".into());
    let (method, sealed) = method_and_payload(bytes).ok_or_else(corrupt)?;
    if method == SECRET_SERVICE_PROTECTOR && protector.id() != method {
        return Err(Error::Storage(KEYRING_NEEDED.into()));
    }
    if method != protector.id() {
        return Err(Error::Storage(format!(
            "the device key is protected with method {method}, but this build uses {}",
            protector.id()
        )));
    }
    let plain =
        protector.unprotect(sealed).map_err(|e| Error::Storage(format!("cannot unlock device key: {e}")))?;
    let bytes: [u8; 32] = plain.as_slice().try_into().map_err(|_| corrupt())?;
    Ok(SecretKey::from_bytes(&bytes))
}

/// Writes via a temporary file and rename, so a crash never leaves a
/// half-written key behind.
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        // A fresh file, private from the start: a leftover (or a link) at
        // the temporary path is removed rather than written through.
        let _ = fs::remove_file(&tmp);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// The install ID a device reports to its peers ([`DeviceInfo::install_id`]): a salted
/// BLAKE3 hash of a platform ID that survives app reinstalls (Android's `ANDROID_ID`,
/// Windows' `MachineGuid`). The raw ID never leaves the device; the salt keeps the hash
/// specific to G7 Link, so it can't be matched against other apps' identifiers.
///
/// [`DeviceInfo::install_id`]: g7link_protocol::messages::DeviceInfo::install_id
pub fn install_id_from(platform_id: &str) -> Option<String> {
    let raw = platform_id.trim();
    if raw.is_empty() {
        return None;
    }
    let mut hasher = blake3::Hasher::new_derive_key("g7link 2026 install id v1");
    hasher.update(raw.as_bytes());
    let hex = hasher.finalize().to_hex();
    Some(hex[..g7link_protocol::messages::INSTALL_ID_LEN].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for a platform protector so tests never touch a real
    /// keyring or Keystore.
    struct TestProtector;

    impl KeyProtector for TestProtector {
        fn id(&self) -> u8 {
            9
        }
        fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>> {
            Ok(plaintext.iter().map(|b| b ^ 0xA5).collect())
        }
        fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>> {
            Ok(ciphertext.iter().map(|b| b ^ 0xA5).collect())
        }
    }

    #[test]
    fn creates_then_reloads_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let protector = TestProtector;
        let first = load_or_create(dir.path(), &protector).unwrap();
        let second = load_or_create(dir.path(), &protector).unwrap();
        assert_eq!(first.public(), second.public());
    }

    #[test]
    fn key_file_does_not_contain_the_plain_key_when_protected() {
        let dir = tempfile::tempdir().unwrap();
        let protector = TestProtector;
        let key = load_or_create(dir.path(), &protector).unwrap();
        let file = fs::read(dir.path().join(FILE_NAME)).unwrap();
        let raw = key.to_bytes();
        assert!(!file.windows(raw.len()).any(|w| w == raw), "plaintext key found on disk");
    }

    /// A protector that doesn't take in plain data keeps refusing a method-`0`
    /// file. Re-sealing it would turn a plain key dropped into the data folder
    /// into this device's trusted identity.
    #[test]
    fn a_plain_key_file_is_not_adopted_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let key = SecretKey::generate();
        write_atomically(&dir.path().join(FILE_NAME), &encode(&key, &PlainKeyProtector).unwrap()).unwrap();
        assert!(matches!(load_or_create(dir.path(), &TestProtector), Err(Error::Storage(_))));
    }

    #[test]
    fn rejects_corrupt_and_mismatched_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"garbage").unwrap();
        assert!(matches!(load_or_create(dir.path(), &PlainKeyProtector), Err(Error::Storage(_))));

        let mut wrong_method = MAGIC.to_vec();
        wrong_method.push(42);
        wrong_method.extend_from_slice(&[0; 32]);
        fs::write(dir.path().join(FILE_NAME), wrong_method).unwrap();
        assert!(matches!(load_or_create(dir.path(), &PlainKeyProtector), Err(Error::Storage(_))));
    }

    #[test]
    fn install_ids_are_stable_salted_and_well_formed() {
        let a = install_id_from("9774d56d682e549c").unwrap();
        assert_eq!(a, install_id_from(" 9774d56d682e549c ").unwrap(), "stable for the same device");
        assert_ne!(a, install_id_from("9774d56d682e549d").unwrap(), "differs per device");
        assert!(g7link_protocol::messages::is_valid_install_id(&a));
        assert!(!a.contains("9774d56d682e549c"), "the raw ID never appears");
        assert_eq!(install_id_from("  "), None);
    }

    #[test]
    fn key_file_has_0600_permissions_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path(), &TestProtector).unwrap();
        let meta = fs::metadata(dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
}
