// SPDX-License-Identifier: MPL-2.0
//! Seals this device's secrets with a key held by the desktop's Secret Service
//! (GNOME Keyring, KWallet and others, over D-Bus).
//!
//! The service holds one random AES-256 key per user. Everything is sealed
//! with it here, so the service is asked once per run and a large clip never
//! crosses the bus.

use std::{
    collections::HashMap,
    fmt::Display,
    io,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use data_encoding::HEXLOWER;
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use zbus::{
    MatchRule,
    blocking::{Connection, MessageIterator, fdo::DBusProxy},
    message::Message,
    zvariant::{DynamicType, OwnedObjectPath, OwnedValue, Value},
};

use crate::identity::{KeyProtector, PlainKeyProtector, SECRET_SERVICE_PROTECTOR};

/// How sealed data starts. Eight bytes, so that nothing stored in the clear
/// (a 32-byte key included) starts this way by chance.
const SEALED_MAGIC: &[u8; 8] = b"G7SEALv1";
const KEY_LEN: usize = 32;

const BUS_NAME: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_INTERFACE: &str = "org.freedesktop.Secret.Collection";
const PROMPT_INTERFACE: &str = "org.freedesktop.Secret.Prompt";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
/// The path the Secret Service uses for "no object".
const NO_OBJECT: &str = "/";
const ITEM_LABEL: &str = "G7 Link data key";

/// How long the service gets to answer, and then the user to unlock it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

type DataKey = [u8; KEY_LEN];
/// The Secret Service's `(oayays)`: session, parameters, value, content type.
type WireSecret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);
/// What `SearchItems` returns: the unlocked items, then the locked ones.
type ItemLists = (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>);

/// AES-256-GCM under the key the Secret Service holds. Sealed data is the
/// magic, a random nonce, then the ciphertext and its tag.
pub(crate) struct SecretServiceProtector {
    key: LessSafeKey,
    rng: SystemRandom,
}

impl SecretServiceProtector {
    pub(crate) fn new(key: &DataKey) -> Result<Self, String> {
        let key = UnboundKey::new(&AES_256_GCM, key).map_err(|_| "the data key was rejected".to_owned())?;
        Ok(Self { key: LessSafeKey::new(key), rng: SystemRandom::new() })
    }
}

fn aad() -> Aad<&'static [u8]> {
    Aad::from(SEALED_MAGIC.as_slice())
}

impl KeyProtector for SecretServiceProtector {
    fn id(&self) -> u8 {
        SECRET_SERVICE_PROTECTOR
    }

    fn protect(&self, plaintext: &[u8]) -> io::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        self.rng.fill(&mut nonce).map_err(|_| io::Error::other("no system randomness"))?;
        let header_len = SEALED_MAGIC.len() + NONCE_LEN;
        let mut out = Vec::with_capacity(header_len + plaintext.len() + AES_256_GCM.tag_len());
        out.extend_from_slice(SEALED_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(plaintext);
        let nonce = Nonce::assume_unique_for_key(nonce);
        let tag = self.key.seal_in_place_separate_tag(nonce, aad(), &mut out[header_len..]);
        let tag = tag.map_err(|_| io::Error::other("can't seal the data"))?;
        out.extend_from_slice(tag.as_ref());
        Ok(out)
    }

    fn unprotect(&self, ciphertext: &[u8]) -> io::Result<Vec<u8>> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "sealed data can't be opened");
        let body = ciphertext.strip_prefix(SEALED_MAGIC.as_slice()).ok_or_else(invalid)?;
        let (nonce, sealed) = body.split_at_checked(NONCE_LEN).ok_or_else(invalid)?;
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| invalid())?;
        let mut buffer = sealed.to_vec();
        let plain_len = self.key.open_in_place(nonce, aad(), &mut buffer).map_err(|_| invalid())?.len();
        buffer.truncate(plain_len);
        Ok(buffer)
    }

    fn is_legacy_plain(&self, stored: &[u8]) -> bool {
        !stored.starts_with(SEALED_MAGIC.as_slice())
    }
}

/// The keyring's protector, or the plain one when the keyring can't be used.
/// Only success is remembered, so a keyring unlocked later is picked up.
pub(crate) fn default_protector() -> Arc<dyn KeyProtector> {
    static READY: OnceLock<Arc<dyn KeyProtector>> = OnceLock::new();
    if let Some(ready) = READY.get() {
        return ready.clone();
    }
    let chosen = select(fetch_data_key());
    if chosen.id() == SECRET_SERVICE_PROTECTOR {
        let _ = READY.set(chosen.clone());
    }
    chosen
}

fn select(key: Result<DataKey, String>) -> Arc<dyn KeyProtector> {
    match key.and_then(|key| SecretServiceProtector::new(&key)) {
        Ok(protector) => Arc::new(protector),
        Err(reason) => {
            tracing::warn!(%reason, "device key is stored without platform protection");
            Arc::new(PlainKeyProtector)
        }
    }
}

enum Step {
    Prompting,
    Done(Result<DataKey, String>),
}

/// Talks to the service from a thread of its own: zbus's blocking calls panic
/// on a tokio thread, and a service that hangs must not hold up the app.
fn fetch_data_key() -> Result<DataKey, String> {
    let (tx, rx) = mpsc::channel();
    let worker = move || {
        let prompting = tx.clone();
        let key = fetch_or_create(&move || {
            let _ = prompting.send(Step::Prompting);
        });
        let _ = tx.send(Step::Done(key));
    };
    thread::Builder::new().name("g7link-keyring".to_owned()).spawn(worker).map_err(text)?;

    let mut wait = ANSWER_TIMEOUT;
    loop {
        match rx.recv_timeout(wait) {
            Ok(Step::Prompting) => wait = PROMPT_TIMEOUT,
            Ok(Step::Done(key)) => return key,
            Err(_) => return Err("the keyring didn't answer in time".to_owned()),
        }
    }
}

fn fetch_or_create(on_prompt: &dyn Fn()) -> Result<DataKey, String> {
    let client = Client::connect()?;
    let (unlocked, locked) = client.search()?;
    if let Some(item) = first_made(&unlocked) {
        return client.read_key(item);
    }
    if locked.is_empty() {
        client.create_key()?;
    } else {
        // Data is already sealed with this key, so it's worth asking the user.
        client.unlock(locked, on_prompt)?;
    }
    // Read back rather than trusting what was just sent: only a key the
    // service really kept may seal anything.
    let (unlocked, _) = client.search()?;
    let item = first_made(&unlocked).ok_or("the keyring didn't keep the data key")?;
    client.read_key(item)
}

/// The same item every time, should two starts have each made one.
fn first_made(items: &[OwnedObjectPath]) -> Option<&OwnedObjectPath> {
    items.iter().min_by_key(|item| (item.as_str().len(), item.as_str().to_owned()))
}

fn item_attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("application", "app.g7link"), ("purpose", "local-data-key")])
}

fn decode_key(stored: &[u8]) -> Result<DataKey, String> {
    let bytes = HEXLOWER.decode(stored).map_err(|_| "the keyring's data key isn't readable".to_owned())?;
    bytes.try_into().map_err(|_| "the keyring's data key has the wrong size".to_owned())
}

fn text(error: impl Display) -> String {
    error.to_string()
}

struct Client {
    conn: Connection,
    session: OwnedObjectPath,
}

impl Client {
    /// Connects and opens a plain session. The session bus is private to this
    /// user, and anything able to read it could ask the service for the key.
    fn connect() -> Result<Self, String> {
        let conn = Connection::session().map_err(|e| format!("no session bus: {e}"))?;
        let body = ("plain", Value::from(""));
        let reply = conn
            .call_method(Some(BUS_NAME), SERVICE_PATH, Some(SERVICE_INTERFACE), "OpenSession", &body)
            .map_err(|e| format!("no Secret Service: {e}"))?;
        let (_, session): (OwnedValue, OwnedObjectPath) = reply.body().deserialize().map_err(text)?;
        Ok(Self { conn, session })
    }

    fn call<B>(&self, path: &str, interface: &str, method: &str, body: &B) -> Result<Message, String>
    where
        B: serde::Serialize + DynamicType,
    {
        self.conn.call_method(Some(BUS_NAME), path, Some(interface), method, body).map_err(text)
    }

    fn search(&self) -> Result<ItemLists, String> {
        let body = (item_attributes(),);
        let reply = self.call(SERVICE_PATH, SERVICE_INTERFACE, "SearchItems", &body)?;
        reply.body().deserialize().map_err(text)
    }

    fn read_key(&self, item: &OwnedObjectPath) -> Result<DataKey, String> {
        let body = (vec![item.clone()], &self.session);
        let reply = self.call(SERVICE_PATH, SERVICE_INTERFACE, "GetSecrets", &body)?;
        let mut secrets: HashMap<OwnedObjectPath, WireSecret> = reply.body().deserialize().map_err(text)?;
        let (_, _, value, _) = secrets.remove(item).ok_or("the keyring didn't return the data key")?;
        decode_key(&value)
    }

    fn create_key(&self) -> Result<(), String> {
        let reply = self.call(SERVICE_PATH, SERVICE_INTERFACE, "ReadAlias", &("default",))?;
        let collection: OwnedObjectPath = reply.body().deserialize().map_err(text)?;
        if collection.as_str() == NO_OBJECT {
            return Err("there is no default keyring".to_owned());
        }
        let body = (COLLECTION_INTERFACE, "Locked");
        let reply = self.call(collection.as_str(), PROPERTIES_INTERFACE, "Get", &body)?;
        let locked: OwnedValue = reply.body().deserialize().map_err(text)?;
        if bool::try_from(locked).map_err(text)? {
            return Err("the keyring is locked".to_owned());
        }

        let mut key = [0u8; KEY_LEN];
        SystemRandom::new().fill(&mut key).map_err(|_| "no system randomness".to_owned())?;
        // As text, which every keyring stores and shows without trouble.
        let value = HEXLOWER.encode(&key).into_bytes();
        let secret: WireSecret = (self.session.clone(), Vec::new(), value, "text/plain".to_owned());
        let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
        properties.insert("org.freedesktop.Secret.Item.Label", Value::from(ITEM_LABEL));
        properties.insert("org.freedesktop.Secret.Item.Attributes", Value::from(item_attributes()));
        // Never replace: what's sealed with an existing key depends on it.
        let body = (&properties, &secret, false);
        let reply = self.call(collection.as_str(), COLLECTION_INTERFACE, "CreateItem", &body)?;
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = reply.body().deserialize().map_err(text)?;
        if prompt.as_str() != NO_OBJECT {
            return Err("the keyring wants confirmation before storing a key".to_owned());
        }
        Ok(())
    }

    fn unlock(&self, items: Vec<OwnedObjectPath>, on_prompt: &dyn Fn()) -> Result<(), String> {
        static ASKED: AtomicBool = AtomicBool::new(false);

        let reply = self.call(SERVICE_PATH, SERVICE_INTERFACE, "Unlock", &(items,))?;
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
            reply.body().deserialize().map_err(text)?;
        if prompt.as_str() == NO_OBJECT {
            return Ok(());
        }
        // Once per run: a dismissed dialog shouldn't return for every later use.
        if ASKED.swap(true, Ordering::Relaxed) {
            return Err("the keyring is locked".to_owned());
        }
        on_prompt();
        self.run_prompt(prompt.as_str())
    }

    fn run_prompt(&self, prompt: &str) -> Result<(), String> {
        let rule = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(PROMPT_INTERFACE)
            .and_then(|b| b.member("Completed"))
            .map(|b| b.build())
            .map_err(text)?;
        let bus = DBusProxy::new(&self.conn).map_err(text)?;
        bus.add_match_rule(rule).map_err(text)?;
        // Listening before asking, so a fast answer isn't missed.
        let messages = MessageIterator::from(&self.conn);
        self.call(prompt, PROMPT_INTERFACE, "Prompt", &("",))?;
        for message in messages {
            let message = message.map_err(text)?;
            let header = message.header();
            let is_answer = header.member().is_some_and(|m| m.as_str() == "Completed")
                && header.path().is_some_and(|p| p.as_str() == prompt);
            if !is_answer {
                continue;
            }
            let (dismissed, _): (bool, OwnedValue) = message.body().deserialize().map_err(text)?;
            return if dismissed { Err("the keyring unlock was dismissed".to_owned()) } else { Ok(()) };
        }
        Err("the session bus closed during the keyring unlock".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    use super::*;
    use crate::{
        Error,
        identity::{FILE_NAME, load_or_create, open_stored, protector_for_data},
    };

    const METHOD_OFFSET: usize = 4;

    fn protector() -> SecretServiceProtector {
        SecretServiceProtector::new(&[7; KEY_LEN]).unwrap()
    }

    fn key_file(dir: &Path) -> Vec<u8> {
        fs::read(dir.join(FILE_NAME)).unwrap()
    }

    /// Accepts old data but can't seal, as when a write to the keyring fails.
    struct CannotSeal;

    impl KeyProtector for CannotSeal {
        fn id(&self) -> u8 {
            SECRET_SERVICE_PROTECTOR
        }
        fn protect(&self, _plaintext: &[u8]) -> io::Result<Vec<u8>> {
            Err(io::Error::other("sealing failed"))
        }
        fn unprotect(&self, _ciphertext: &[u8]) -> io::Result<Vec<u8>> {
            Err(io::Error::other("opening failed"))
        }
        fn is_legacy_plain(&self, _stored: &[u8]) -> bool {
            true
        }
    }

    #[test]
    fn seals_and_opens_again() {
        let protector = protector();
        let sealed = protector.protect(b"a clipboard secret").unwrap();
        assert!(sealed.starts_with(SEALED_MAGIC.as_slice()));
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        assert_eq!(protector.unprotect(&sealed).unwrap(), b"a clipboard secret");
        assert_ne!(sealed, protector.protect(b"a clipboard secret").unwrap(), "a fresh nonce each time");
    }

    #[test]
    fn empty_data_round_trips() {
        let protector = protector();
        assert!(protector.unprotect(&protector.protect(b"").unwrap()).unwrap().is_empty());
    }

    #[test]
    fn refuses_tampered_short_unsealed_and_foreign_data() {
        let protector = protector();
        let sealed = protector.protect(b"payload").unwrap();

        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(protector.unprotect(&tampered).is_err());

        assert!(protector.unprotect(&sealed[..SEALED_MAGIC.len() + 3]).is_err());
        assert!(protector.unprotect(&sealed[..SEALED_MAGIC.len() + NONCE_LEN + 2]).is_err());
        assert!(protector.unprotect(b"{\"plain\":true}").is_err(), "only open_stored takes unsealed data");

        let other = SecretServiceProtector::new(&[8; KEY_LEN]).unwrap();
        assert!(other.unprotect(&sealed).is_err());
    }

    #[test]
    fn open_stored_flags_data_from_before_sealing() {
        let protector = protector();
        let (plain, was_plain) = open_stored(&protector, b"{\"items\":[]}").unwrap();
        assert_eq!(plain, b"{\"items\":[]}");
        assert!(was_plain);

        let sealed = protector.protect(b"new").unwrap();
        let (plain, was_plain) = open_stored(&protector, &sealed).unwrap();
        assert_eq!(plain, b"new");
        assert!(!was_plain);
    }

    #[test]
    fn select_uses_the_keyring_only_when_it_answered() {
        assert_eq!(select(Ok([1; KEY_LEN])).id(), SECRET_SERVICE_PROTECTOR);
        assert_eq!(select(Err("no session bus".to_owned())).id(), 0);
    }

    #[test]
    fn keys_from_the_keyring_are_validated() {
        let key = [0xab; KEY_LEN];
        assert_eq!(decode_key(HEXLOWER.encode(&key).as_bytes()).unwrap(), key);
        assert!(decode_key(b"not hex").is_err());
        assert!(decode_key(HEXLOWER.encode(&[1; 16]).as_bytes()).is_err());
    }

    #[test]
    fn the_first_made_item_is_chosen_whatever_the_order() {
        let path = |p: &str| OwnedObjectPath::try_from(p).unwrap();
        let items = [path("/c/login/10"), path("/c/login/9"), path("/c/login/100")];
        assert_eq!(first_made(&items).unwrap().as_str(), "/c/login/9");
        assert!(first_made(&[]).is_none());
    }

    #[test]
    fn a_new_key_is_sealed_when_the_keyring_answers() {
        let dir = tempfile::tempdir().unwrap();
        let key = load_or_create(dir.path(), &protector()).unwrap();
        let file = key_file(dir.path());
        assert_eq!(file[METHOD_OFFSET], SECRET_SERVICE_PROTECTOR);
        let raw = key.to_bytes();
        assert!(!file.windows(raw.len()).any(|w| w == raw));
        assert_eq!(load_or_create(dir.path(), &protector()).unwrap().public(), key.public());
    }

    #[test]
    fn without_a_keyring_the_key_stays_a_private_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = select(Err("locked".to_owned()));
        let key = load_or_create(dir.path(), fallback.as_ref()).unwrap();
        assert_eq!(key_file(dir.path())[METHOD_OFFSET], 0);
        let mode = fs::metadata(dir.path().join(FILE_NAME)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(load_or_create(dir.path(), fallback.as_ref()).unwrap().public(), key.public());
    }

    #[test]
    fn an_existing_plain_key_is_sealed_and_stays_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = load_or_create(dir.path(), &PlainKeyProtector).unwrap();

        let loaded = load_or_create(dir.path(), &protector()).unwrap();
        assert_eq!(loaded.public(), key.public());

        let file = key_file(dir.path());
        assert_eq!(file[METHOD_OFFSET], SECRET_SERVICE_PROTECTOR);
        let raw = key.to_bytes();
        assert!(!file.windows(raw.len()).any(|w| w == raw), "the plain key is gone from the file");
        let mode = fs::metadata(dir.path().join(FILE_NAME)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(load_or_create(dir.path(), &protector()).unwrap().public(), key.public());
    }

    #[test]
    fn a_failed_migration_leaves_the_plain_key_loadable() {
        let dir = tempfile::tempdir().unwrap();
        let key = load_or_create(dir.path(), &PlainKeyProtector).unwrap();
        let before = key_file(dir.path());

        let loaded = load_or_create(dir.path(), &CannotSeal).unwrap();
        assert_eq!(loaded.public(), key.public());
        assert_eq!(key_file(dir.path()), before);
        assert_eq!(protector_for_data(dir.path(), Arc::new(CannotSeal)).id(), 0, "nothing else gets sealed");
    }

    #[test]
    fn a_plain_key_others_can_read_loads_but_is_not_sealed() {
        let dir = tempfile::tempdir().unwrap();
        let key = load_or_create(dir.path(), &PlainKeyProtector).unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(load_or_create(dir.path(), &protector()).unwrap().public(), key.public());
        assert_eq!(key_file(dir.path())[METHOD_OFFSET], 0);
    }

    #[test]
    fn a_sealed_key_is_left_alone_when_it_cannot_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path(), &protector()).unwrap();
        let before = key_file(dir.path());

        match load_or_create(dir.path(), select(Err("locked".to_owned())).as_ref()) {
            Err(Error::Storage(message)) => assert!(message.contains("keyring"), "{message}"),
            other => panic!("expected a storage error, got {:?}", other.map(|k| k.public())),
        }
        assert_eq!(key_file(dir.path()), before);

        let other_key = SecretServiceProtector::new(&[8; KEY_LEN]).unwrap();
        assert!(matches!(load_or_create(dir.path(), &other_key), Err(Error::Storage(_))));
        assert_eq!(key_file(dir.path()), before);
    }

    #[test]
    fn a_sealed_file_holding_plain_bytes_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path(), &PlainKeyProtector).unwrap();
        let mut forged = key_file(dir.path());
        forged[METHOD_OFFSET] = SECRET_SERVICE_PROTECTOR;
        fs::write(dir.path().join(FILE_NAME), &forged).unwrap();

        assert!(matches!(load_or_create(dir.path(), &protector()), Err(Error::Storage(_))));
        assert_eq!(key_file(dir.path()), forged);
    }

    #[test]
    fn other_data_is_sealed_only_once_the_device_key_is() {
        let dir = tempfile::tempdir().unwrap();
        let sealing: Arc<dyn KeyProtector> = Arc::new(protector());

        load_or_create(dir.path(), &PlainKeyProtector).unwrap();
        assert_eq!(protector_for_data(dir.path(), Arc::new(PlainKeyProtector)).id(), 0);

        load_or_create(dir.path(), sealing.as_ref()).unwrap();
        assert_eq!(protector_for_data(dir.path(), sealing).id(), SECRET_SERVICE_PROTECTOR);
    }
}
