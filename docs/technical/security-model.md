# Security Model

G7 Link uses an end-to-end encrypted, mutually authenticated peer-to-peer security model with no central user accounts and no cloud storage. Every connection is authenticated at the transport layer by static Ed25519 device identities and authorized at the application layer by per-device capability toggles and interactive runtime consent gates.

For the full audit record and vulnerability remediation history, see [`docs/SECURITY-REVIEW.md`](../SECURITY-REVIEW.md).

## Threat Model

| Adversary / Environment | Assumed Capabilities | Mitigations |
|---|---|---|
| Untrusted LAN / Public Wi-Fi | Can eavesdrop, replay, inject, or MITM UDP/TCP packets; can broadcast spoofed mDNS records | QUIC + TLS 1.3 (`iroh`) with mutual Ed25519 certificate verification; pairing requires either an out-of-band 128-bit QR secret (`HMAC-SHA256`) or a commitment-bound 6-digit SAS confirmed on both screens |
| Compromised Relay Server (**Connect over mobile data** / `away_mode`) | Can observe outer UDP packet sizes/timing and drop traffic | End-to-end TLS 1.3 termination between peer `DeviceId` keys; relay never possesses session keys; `storage` without `remote_files` is blocked on `ConnectionPath::Relay` |
| Malicious Web Page on Local PC | Can send cross-origin `fetch` / `XMLHttpRequest` / `WebSocket` requests to `127.0.0.1:53320` | Local HTTP API is disabled by default; requires a 256-bit Bearer token in the `Authorization` header (constant-time verified) and rejects any request carrying an `Origin` header |
| Unprivileged Local App on Android | Can attempt to connect to local ports or read app storage | Elevated helper (`InputServer`) binds `127.0.0.1:<ephemeral_port>` and requires a 192-bit (24-byte / 48-hex-char) per-session `SecureRandom` token on the first line, verified in constant time (`MessageDigest.isEqual`) before accepting any commands |
| Another App Reaching G7 Link's Shizuku Service (Elevated through Shizuku, `elevated/ShizukuShell.kt`) | Gets hold of the Binder of the process Shizuku runs for G7 Link, as the shell user, and asks it to run commands | Shizuku hands that Binder only to G7 Link, and `ShizukuShell` answers only G7 Link's own UID and Shizuku's. It runs nothing it's given as a command line: only the grants Elevated needs for G7 Link's own package (`pm grant`, `appops set … allow`, `cmd notification allow_listener|allow_dnd`, checked word by word) as argument lists, and `InputServer` from G7 Link's own APK with a 48-hex token. It isn't used when Shizuku runs as root, and its process ends with G7 Link's |
| Offline Disk Inspection | Read access to the data folder (`~/.local/share/G7Link` or Android's app data directory) | On Linux, the private identity key (`identity.key`), clipboard history, the LocalSend identity and queued sends are sealed with AES-256-GCM under a random key held by the desktop's Secret Service (GNOME Keyring, KWallet). On Android they are sealed with Android Keystore AES-256-GCM (hardware-backed when the device supports it). **With no keyring on Linux** they are stored unsealed in `0600` files, so use full-disk encryption if this matters to you |
| Paired Device Exceeding Intended Access | Attempts to invoke disabled capabilities, traverse outside shared paths, or start silent screen/input capture | Inbound dispatch enforces `CapabilityMatrix::is_allowed` per peer, canonicalizes file paths, and gates sensitive sessions (`pc_mirror`, `remote_input`, first-time `storage`, `webcam`) behind explicit user confirmation |

## Cryptographic Device Identity

Implemented in `core/g7link-core/src/identity.rs`.

- Each installation generates a 32-byte Ed25519 secret key (`iroh::SecretKey`) using the OS CSPRNG (`rand::rng()`).
- The corresponding 32-byte Ed25519 public key (`iroh::EndpointId`) serves as the canonical `DeviceId` (rendered in logs and URIs as z-base-32, with a 16-character `short()` prefix for UI display).
- On disk, `identity.key` is stored with a 5-byte header:
  - Bytes `0..4`: Signature `NLK1` (`b"NLK1"`).
  - Byte `4`: `KeyProtector` ID (`0` = `PlainKeyProtector`, used when no keyring is available and by the headless CLI and tests; `2` = Android Keystore; `3` = Secret Service; `1` is retired).
  - Bytes `5..`: Sealed ciphertext.
- **Linux Secret Service Protection (`SecretServiceProtector`, `core/g7link-core/src/keyring.rs`)**: Keeps one random 256-bit data key per user as an item (label `G7 Link data key`, attributes `application = app.g7link`, `purpose = local-data-key`) in the default collection of the desktop's Secret Service (`org.freedesktop.secrets` over D-Bus: GNOME Keyring, KWallet and others), created on first use. Every payload is sealed with AES-256-GCM (`ring`) as `b"G7SEALv1" || 12-byte nonce || ciphertext || tag`, with the magic as associated data. The service is asked once per run, so large clips never cross the bus. A copy of the data folder without the keyring item can't be opened.
- **Locked or missing keyring**: When the item exists but its collection is locked, G7 Link asks the service to unlock it and waits up to two minutes for the user. When there is no Secret Service, or the collection is locked and holds no G7 Link item yet, it uses `PlainKeyProtector`: `identity.key` is written with `0600` permissions, unsealed, and the core logs `device key is stored without platform protection`. Other data is sealed only once the device key is, so a run that can't reach the keyring stops at the key (with an error asking to unlock the keyring) instead of reading sealed files as plain.
- **Android Keystore Protection (`KeystoreKeyProtector.kt`)**: Generates a non-exportable 256-bit AES-GCM key (`alias = "g7link.identity.v1"`) inside `AndroidKeyStore` (`PURPOSE_ENCRYPT | PURPOSE_DECRYPT`, `BLOCK_MODE_GCM`, `ENCRYPTION_PADDING_NONE`, which may be hardware-backed on devices with StrongBox or TEE) and seals secrets as `12-byte IV || ciphertext || 128-bit auth tag`.
- Legacy raw 32-byte key files are migrated into the `NLK1` format on first load. On Linux, an unsealed (method `0`) key is sealed in place once the keyring is available, keeping the existing `DeviceId` (the device does not have to pair again), but only when the file is a regular file owned by the user with no group or other permissions: a plain key dropped into the data folder by someone else is never adopted as the device's identity. The sealed file is opened again and compared before it replaces the plain one.

## Pairing Protocols (`ALPN_PAIR = "g7link-pair/0"`)

Implemented in `core/g7link-protocol/src/pairing.rs` and `core/g7link-core/src/pairing.rs`. Pairing runs over a dedicated QUIC ALPN (`g7link-pair/0`) and enforces a 5-minute expiration (`PAIRING_TTL = 300s`) and a maximum of 5 failed attempts (`MAX_FAILED_ATTEMPTS = 5`) before invalidating the pairing session.

### 1. QR Code Pairing Flow

```text
Host (displays QR)                                      Joiner (scans QR)
------------------                                      -----------------
Generates 128-bit secret S (16 bytes)
Encodes g7link://pair?v=0&id=<host_id>&s=<base64url_S>&n=<name>&a=<ip:port>

                  ◄─── QUIC + TLS 1.3 (ALPN: g7link-pair/0) ───
                  ◄─── pair.request { id, name, kind, proof } ─────
                       where proof = HMAC-SHA256(S,
                         "g7link-pair-v0" || joiner_id || host_id)

Verifies TLS peer == joiner_id
Verifies HMAC proof in constant time
                  ──── pair.accept { id, name, kind } ────────────►
                  ◄─── pair.done ──────────────────────────────────
Persists peer to SQLite                                 Persists peer to SQLite
Closes with CLOSE_PAIRED (0)
```

- Because the 128-bit secret `S` is transferred optically via the QR code (`s` encoded in URL-safe base64 without padding) and the HMAC binds both `joiner_id` and `host_id` verified against the TLS 1.3 peer certificates, an active network attacker cannot forge `pair.request` or substitute their own `DeviceId`.

### 2. Nearby Discovery and 6-Digit Code Flow

When pairing via mDNS discovery or manual IP without scanning a QR code, both devices execute a commitment-based Short Authentication String (SAS) ceremony:

1. **Initiation**: Joiner sends `pair.request` with `proof: None`.
2. **Commitment Exchange**:
   - Joiner (initiator `A`) generates a 128-bit random nonce `nA` and sends `pair.commit { commit: SHA-256(nA) }`.
   - Host (responder `B`) generates a 128-bit random nonce `nB` and replies with `pair.nonce { nonce: nB }`.
   - Joiner reveals `pair.reveal { nonce: nA }`.
   - Host verifies `SHA-256(nA) == commit` in constant time (`subtle::ConstantTimeEq`).
3. **SAS Calculation**: Both sides compute a 6-digit numeric code:
   - `SAS = (u32_be(HMAC-SHA256(nA || nB, "g7link-sas-v0" || idA || idB)[0..4]) & 0x7fff_ffff) mod 1,000,000` (where `idA` is the initiator's 32-byte Ed25519 public key and `idB` is the responder's). The IDs are the ones TLS verified, so a man in the middle, who holds a different key, changes the input on at least one side.
4. **Two-Sided User Confirmation**: Both devices display the 6-digit code and require explicit user approval (`pair.confirm { accept: true }`). Neither side writes the peer to SQLite until `pair.done` completes the two-phase commit.

## Session Authentication (`ALPN_SESSION = "g7link/0"`)

Every operational session connects over `ALPN_SESSION` (`"g7link/0"`):

1. During the QUIC + TLS 1.3 handshake, `iroh` cryptographically verifies that the remote peer holds the private key for its claimed `EndpointId` (`DeviceId`).
2. Immediately upon connection acceptance (`node.rs`), the node looks up the remote `DeviceId` in the SQLite `peers` table (`store.is_paired(&peer_id)`).
3. If the `DeviceId` is not present in `peers`, the connection is immediately terminated with application close code `CLOSE_UNPAIRED` (`1`) before any control stream or capability handler is invoked.

## Authorization and the `CapabilityMatrix`

Implemented in `core/g7link-core/src/features.rs`. Every paired device has an independent set of 19 feature toggles stored in the `peer_toggles` SQLite table (`DEVICE_TOGGLES`).

### Default-On vs Opt-In Toggles

| Category | Toggle Keys | Default State |
|---|---|---|
| Standard Continuity (14) | `notifications`, `messages`, `calls`, `contacts`, `clipboard`, `files`, `media`, `photos`, `recordings`, `toggles`, `pc_actions`, `mirroring`, `webcam`, `mic` | Enabled (`true`) upon pairing |
| High-Sensitivity / Opt-In (5) | `storage`, `pc_mirror`, `remote_input`, `commands`, `remote_files` | Disabled (`false`) upon pairing; must be explicitly enabled by the user |

### Inbound Enforcement

Each service's handler checks the peer's persisted toggle (`Shared::toggle_on`) before doing anything, and denies the request when it's off. `CapabilityMatrix` in `features.rs` works out what each device can offer, for the UI and for what's advertised.

### Relay Restrictions (**Connect over mobile data** / `away_mode`)

When a session is connected over `ConnectionPath::Relay` (**Connect over mobile data** / `away_mode`), `Node` applies additional transport-aware authorization:

- Remote file system browsing and downloads are blocked on relay connections unless the user has enabled **both** the `storage` toggle and the opt-in `remote_files` toggle for that peer.
- With **Connect over mobile data** (`away_mode`) off, the relay map is empty and no internet address lookup runs, so nothing is published or relayed. The switch applies while running, both ways (`apply_internet_services`). Turning it off closes non-LAN sessions, removes the relays and lookups, and refuses relay paths. iroh keeps its idle connection to the current home relay until the endpoint closes, so that one connection, carrying nothing, lasts until the next restart.
- What's published while it's on holds the device's addresses, not its name (`WithoutName`).

## Runtime Consent Gates

Even when a capability toggle is enabled, sensitive operations require interactive user confirmation:

1. **Reverse PC Screen Mirroring**: When a phone requests to view the PC screen, the desktop app emits `NodeEvent::PcMirrorRequested` and blocks streaming until the user approves the prompt (`set_pc_mirror_session_allowed`). While active, the desktop displays a persistent top banner with a one-click **Stop Sharing** button (`stop_pc_mirror`).
2. **Remote Input Control**: Synthesizing mouse and keyboard input on the PC requires the opt-in `remote_input` toggle on the PC. A phone asking while it's off prompts the PC once (`NodeEvent::RemoteInputRequested`).
3. **Phone Storage Browser**: It needs the `storage` toggle on both devices, and Android's own storage permission on the phone: All files access, or the folders you picked.
4. **Phone-as-Webcam and Phone as microphone (`mic`)**: Starting either from the PC needs a tap on the phone, except for a PC chosen under **Start the camera and microphone without asking** (`Elevated`, off by default). The phone can stop them at any time.
5. **LocalSend Incoming Files**: Unauthenticated LAN LocalSend peers never bypass pairing; every incoming LocalSend offer enters `TransferState::Requested` and waits for explicit user acceptance before writing files to the Downloads directory.

## File System and Path Traversal Protections

- **Incoming Transfers (`core/g7link-core/src/transfer.rs`)**: `is_valid_file_name` and `is_valid_folder` reject separators in names, control characters, `.` and `..`. `safe_file_name` keeps only the last path component, and `storable_name` also replaces characters and names other systems can't store (`<>:"|?*`, trailing dots and spaces, reserved device names), segment by segment, so writes stay inside the download folder.
- **Storage Browser (`android/app/.../storage/PhoneStorage.kt`)**: Resolves every requested path canonically and checks it's inside the storage root and outside the blocked folders, so symlinks can't escape. `/data`, `Android/data`, `Android/obb` and G7 Link's own folders are never exposed.

## Automatic Updates and Release Verification

Implemented in `desktop/app/src/updater.rs` (with `feed.rs`, `update_relay.rs` and the Linux HTTPS client) and `android/app/src/main/kotlin/app/g7link/android/update/AppUpdater.kt`.

- **Endpoint**: Both apps read their channel's feed, `https://raw.githubusercontent.com/Astraque-Softwares-Ltd/g7link/channels/<stable|nightly>.json`, written by the release and nightly workflows, with a conditional request (`If-None-Match`). Until a feed is published they query `https://api.github.com/repos/Astraque-Softwares-Ltd/g7link/releases/latest` instead. Both over HTTPS with the system's TLS stack (`curl`, falling back to `wget`, on Linux, so system CA certificates and proxy settings apply; `HttpURLConnection` on Android). Only `https://` URLs are fetched, redirects stay on HTTPS, and responses are size-capped while they are read. No device identifiers or telemetry are sent.
- **Feeds**: a feed is accepted only with schema 1, the channel it was fetched for, and a version of that channel (a stable feed lists releases only, a nightly one nightlies only). Its files are kept only when their address starts with `https://github.com/Astraque-Softwares-Ltd/g7link/releases/download/`, their name is a plain file name, and they carry a 64-digit SHA-256; the download is rejected unless it matches. So a feed can't point an updater anywhere but this repository's own release files, which is the same trust as the release listing it replaces.
- **Hints between devices** (`event.update`): a paired device may tell another that a build is out, so a phone needn't poll. A hint only starts a check of the receiver's own feed (at most every few minutes); a hint never names a file or hash, so a compromised peer can't make a device install anything it wouldn't have found itself.
- **What is accepted**: a build newer than the running one on the install's channel (Stable: releases; Nightly: nightlies and releases), with a file of the install's own kind (an AppImage for an AppImage, `_amd64.deb` for a `.deb` install, and so on, or `*-android.apk`), and its SHA-256 from the feed (or, through the API, a published release's `SHA256SUMS.txt` that lists it). Android additionally refuses an APK not signed with the app's own key.
- **Linux, installing**: only an installed copy updates itself, and only when the user clicks **Update**. The app downloads the file as the user (resumable, with progress) into a folder only the user can write, and checks its SHA-256. An AppImage is replaced in place. A `.deb`, an `.rpm`, or a tarball under `/opt` or `/usr` is installed through `pkexec`, which asks for the administrator password: a fixed script copies the file into `/var/cache/g7link-update` (created `0700`, owned by root), checks the copy against the expected hash again, and installs it with `apt-get`/`dpkg`, `dnf`/`zypper`/`rpm`, or by unpacking the tarball. Release `.rpm`s aren't GPG-signed, so their SHA-256 is what's checked. A tarball in the user's own folders is unpacked and installed as the user. A small relauncher then opens the new version.
- **Android**: with **Install updates automatically** on, the connection service checks in the background at most once a day and installs through `PackageInstaller` with `USER_ACTION_NOT_REQUIRED` (Android 12+, `UPDATE_PACKAGES_WITHOUT_USER_ACTION`). Android skips the prompt only when G7 Link installed the current version itself; otherwise the user confirms from a notification.
- **Opt-out**: **Check for updates** in Settings on the desktop, and **Check for updates** and **Install updates automatically** on Android. With checks off, nothing is fetched.
- **Not yet**: the Linux packages are not signed, so trust rests on HTTPS to GitHub and the release's checksums.

## Localhost HTTP API Isolation

Implemented in `desktop/app/src/local_api.rs`.

- **Default State**: Disabled (`local_api_enabled: false`).
- **Network Binding**: Binds strictly to `127.0.0.1:53320` (`DEFAULT_API_PORT`), never `0.0.0.0`.
- **Token Authentication**: Generates a 32-byte (64-hex-char) random token from the system CSPRNG (`secure_random`, reading `/dev/urandom`; the API stays off if it can't be read) stored in `<data_dir>/api-token.json`. Every request must supply `Authorization: Bearer <token>`, checked using a constant-time byte comparison (`constant_time_eq`).
- **Browser Drive-By Defense**: Any request with an `Origin` header gets `403 Forbidden` before routing, and a page can't send the `Authorization` header without the token anyway.
