# Storage and Retention

G7 Link persists all relational state in an embedded SQLite database (`g7link.db`) managed by `core/g7link-core/src/store.rs`, supplemented by a `KeyProtector`-encrypted clipboard vault (`core/g7link-core/src/clipboard_history.rs`) and bounded desktop notification state files (`desktop/app/src/notification_store.rs`).

## Data Directory Layout

- **Linux (`~/.local/share/G7Link`)** or custom `--data-dir`
- **Android (`context.filesDir`)**

```text
<data_dir>/
├── identity.key                  Ed25519 secret key (NLK1 header + Secret Service/Keystore ciphertext)
├── g7link.db                     SQLite database (WAL mode, schema v11)
├── clipboard_history.enc         KeyProtector-encrypted clipboard index (NLCB\x01 header)
├── clipboard_images/
│   └── <clip_id>.enc             KeyProtector-encrypted PNG/image payloads (NLCI\x01 header)
├── incoming/
│   └── <peer_id>/<transfer_id>/  In-flight .part files for resumable transfers
├── queue_staging/
│   └── <queue_id>/               Staged copies of ephemeral Android ContentResolver FDs
├── notification-rules.json       Per-app notification mute/banner rules (desktop)
├── notification-history.json     24-hour notification history cache (desktop)
└── api-token.json                256-bit Bearer token for opt-in localhost HTTP API (desktop)
```

## SQLite Schema (`PRAGMA user_version = 11`)

Configured in `core/g7link-core/src/store.rs` with `PRAGMA journal_mode = WAL`, `PRAGMA foreign_keys = ON`, and `PRAGMA user_version` set to the number of entries in `MIGRATIONS` (11). `peers`, `peer_toggles`, `timeline` and `meta` use SQLite `STRICT` typing; `chat_threads`, `chat_messages` and `send_queue` do not:

| Table | Primary Key | Purpose | Retention & Bounding Policy |
|---|---|---|---|
| `meta` | `key TEXT` | Local device name (`device_name`) and global key-value settings | Permanent per installation |
| `peers` | `id BLOB` | Paired devices (32-byte Ed25519 `id`, `name`, `kind`, `paired_at_ms`, `last_seen_ms`, `last_addrs`) | Removed immediately upon `unpair` (`ON DELETE CASCADE` cleans child rows) |
| `peer_toggles` | `(peer, toggle)` | Per-device capability toggle overrides (`peer BLOB`, `toggle TEXT`, `enabled INTEGER`) | Cascade-deleted when parent `peers` row is removed |
| `timeline` | `id INTEGER` | Unified cross-device activity log (`ts_ms`, `peer BLOB`, `kind`, `direction`, `summary`, `payload BLOB`) | Indexed by `ts_ms DESC`; supports substring filtering (`search_timeline`) and clear actions |
| `chat_threads` | `(peer TEXT, thread_id TEXT)` | App chats from `MessagingStyle` notifications, one row per conversation; see [Unified Conversations](#unified-conversations-chat_threads--chat_messages) | Live while the notification is on the phone; a cleared chat goes after 24 hours unless kept, a kept one after 30 idle days; at most `MAX_CHAT_THREADS_PER_PEER = 200` per phone |
| `chat_messages` | `id INTEGER` | The messages of a `chat_threads` row (`peer`, `thread_id`, `sender`, `text`, `time_ms`, `self_sent`, `avatar`) | At most `MAX_CHAT_MESSAGES_PER_THREAD = 200` per thread; deleted with their thread |
| `send_queue` | `id TEXT` | Persistent bidirectional FIFO outbound delivery queue (`seq`, `peer BLOB`, `kind`, `status`, `created_at_ms`, `expires_at_ms`, `retry_count`, `next_retry_ms`, `last_error`, `total_bytes`, `sent_bytes`, `transfer_id`, `summary`, `payload_json`) | Active items capped at `200` items / `10 GiB`; terminal history pruned at `100` rows (`MAX_QUEUE_HISTORY_ITEMS`) |

### Schema Migrations (`Store::migrate`)

`Store::open` reads `PRAGMA user_version` and applies each missing entry of `MIGRATIONS` (`v1` → … → `v11`) in its own transaction, setting `user_version` in the same transaction. It refuses a database from a newer version. Version 8 added `peers.install_id`: a salted hash of the other device's Android ID or Linux `/etc/machine-id`, used only to replace the old entry when the same device is paired again (`cleanup_peers` merges by install ID and device kind, never by name). Version 10 added the sealed `peers.link_secret`. Version 11 keys chats by conversation; see below.

## Unified Conversations (`chat_threads` & `chat_messages`)

When an Android notification uses `Notification.MessagingStyle` (WhatsApp, Signal, Telegram, Google Messages, Discord, Slack and others), `NotificationReader.kt` sends its conversation with the notification: the conversation's hashed ID when the app gave it one, its title, whether it's a group, up to 25 messages, and the reply action (see [notifications §2.3, §2.4](../protocol/notifications.md)). The PC stores it in `chat_threads` and `chat_messages`. SMS is not stored here: the PC reads SMS threads from the phone's provider when it shows them.

### Columns

`chat_threads`, primary key `(peer, thread_id)`:

| Column | Meaning |
|---|---|
| `peer` | The phone's device ID (text) |
| `thread_id` | `chat:<app>:i:<conversation ID>`, or `chat:<app>:t:<title without its count, lowercased>` (`chat_thread_id`) |
| `app`, `app_name` | Package name and the app's name |
| `notification_key` | The newest notification of the conversation; used to retire it, never as its identity |
| `reply_action_id` | The notification's reply action, if it has one |
| `title` | The conversation's title without a message count; the newest one wins, so a renamed group stays one row |
| `is_group`, `snippet`, `last_sender`, `updated_ms` | From the newest notification (`updated_ms` is the phone's message time) |
| `unread` | Incoming messages stored since the chat was last opened, replied to or retired on the PC |
| `active` | 1 while the notification is on the phone ("live") |
| `avatar`, `app_icon` | Pictures, kept from earlier notifications when a newer one has none |
| `conv_id` | The phone's conversation ID, when it sent one (v11) |
| `kept` | 1 once replied to from this PC, or pinned (v11) |
| `retired_ms` | When the notification left the phone, on this PC's clock; NULL exactly while `active = 1` (v11) |

`chat_messages` holds each message once per thread: `peer`, `thread_id`, `sender` (NULL when sent from the phone's side), `text`, `time_ms`, `self_sent`, `avatar`.

### How a chat is stored

- `Store::upsert_chat_notification` (each `notify.posted`) and `Store::apply_chat_snapshot` (each `notify.snapshot`, in one transaction) store a conversation under its thread, insert the messages it doesn't have yet, and make it live (`active = 1`, `retired_ms = NULL`). `unread` grows by the incoming messages newly inserted, so a re-post adds nothing; a chat first seen in a snapshot counts at most the messages its notification lists. `kept` is never changed here.
- A thread first stored by its title takes its conversation ID when a notification first brings one: the row and its messages move to the `i:` thread, with `kept`.
- `notify.removed` retires every live row with that notification key (`Store::mark_chat_notification_removed`): `active = 0`, `unread = 0`, `retired_ms = now`.
- A snapshot that isn't `truncated` also retires every live row whose notification key is in none of its items. Notifications turned off for this PC send an empty snapshot, which retires everything.
- A reply sent from the PC (`append_chat_reply`) is stored as a `self_sent` message and sets `kept = 1`; `Node::set_chat_kept` pins and unpins. A message sent on the phone that shows in the notification doesn't keep a chat.
- `Node::remove_chat_thread` deletes the thread and its messages, then dismisses its notification on the phone when it was live. `Node::chat_threads` lists rows with `active = 1 OR kept = 1`; `Node::chat_thread` finds any stored row.

### Lifetime (`prune_chat_conn`)

Runs after each stored notification, removal and snapshot, and once at startup (`Store::prune_chats`), on this PC's clock:

| State | Listed | Deleted when |
|---|---|---|
| live (`active = 1`) | yes | never by age |
| retired, not kept | no | 24 hours after `retired_ms` (`CHAT_RETIRED_GRACE_SECS`) |
| retired, kept | yes | 30 days after the later of `updated_ms` and `retired_ms` (`CHAT_KEPT_IDLE_SECS`) |

The 24 hours only let a notification listener that rebinds (an empty snapshot, then a full one) bring a chat back with its messages. Beyond these, the PC keeps at most `MAX_CHAT_THREADS_PER_PEER = 200` threads per phone (the newest by `updated_ms`) and at most `MAX_CHAT_MESSAGES_PER_THREAD = 200` messages per thread, applied as each thread is stored. A thread's messages are deleted in the same transaction as the thread.

### Migration to v11

v11 adds `conv_id`, `kept`, `retired_ms` and an index on `(peer, notification_key)`, then, in the same transaction, `merge_chat_rows`: rows of one phone, app and title without its count become one `chat:<app>:t:<title>` row with the newest row's header, the newest avatar and icon, and every message of the group, exact duplicates (same sender, text, time and `self_sent`) stored once. A group is kept when it holds a `self_sent` message (v10 couldn't tell a reply from the PC from one sent on the phone). The merge checks that the messages left are exactly the distinct ones it started with, and that no two groups share a thread; if not, it is undone and the rows stay as they were. Either way every row is then retired with `unread = 0`, so the first snapshot after updating brings back the chats that are on the phone. It writes nothing outside `chat_threads` and `chat_messages`.

## Desktop Notification History (`desktop/app/src/notification_store.rs`)

In addition to SQLite conversation threads, the desktop app maintains fast JSON-backed stores for notification rules and recent notifications:

- `notification-rules.json`: Stores `Rules { history: bool, apps: BTreeMap<String, AppEntry> }` where each `AppEntry` maps `<package_name>` to `AppRule` (`Show`, `Quiet`, or `Hidden`).
- `notification-history.json`: Retains up to `MAX_HISTORY = 300` history entries (alongside up to `MAX_NOTIFICATIONS = 200` active notifications in memory) and automatically prunes entries older than 24 hours (`HISTORY_RETENTION_MS = 24 * 60 * 60 * 1000`) on load and save.

## Persistent Bidirectional Send Queue (`send_queue`)

Implemented across `core/g7link-core/src/store.rs`, `core/g7link-core/src/node.rs`, and `core/g7link-core/src/transfer.rs`.

Whenever a user sends files, folders, links, clipboard payloads, voice recordings, handoff documents, task alerts, or camera requests while the target device is offline (or when the connection drops mid-operation), G7 Link enqueues the action into `send_queue` and delivers it automatically when a session is established.

### Queue Item Kinds and Default TTLs (`QueueKind`)

| `QueueKind` | Wire Value | Default TTL | Description |
|---|---|---|---|
| `QueueKind::Files` | `"files"` | 7 days (`604,800,000 ms`) | One or more files (`QueuePayload::Files`) |
| `QueueKind::Folder` | `"folder"` | 7 days (`604,800,000 ms`) | Directory tree with relative paths (`QueuePayload::Files { is_folder: true }`) |
| `QueueKind::HandoffDocument` | `"handoff_document"` | 7 days (`604,800,000 ms`) | Document file + reading position (`page`, `scroll_y`) |
| `QueueKind::Recording` | `"recording"` | 7 days (`604,800,000 ms`) | Voice or audio recording file |
| `QueueKind::Clipboard` | `"clipboard"` | 24 hours (`86,400,000 ms`) | Text clipboard payload (`QueuePayload::ClipboardText`) |
| `QueueKind::ClipboardImage` | `"clipboard_image"` | 24 hours (`86,400,000 ms`) | Image clipboard payload (`QueuePayload::ClipboardImage`) |
| `QueueKind::Link` | `"link"` | 24 hours (`86,400,000 ms`) | URL + optional title (`QueuePayload::Link`) |
| `QueueKind::TaskNotify` | `"task_notify"` | 24 hours (`86,400,000 ms`) | Long-running command/task completion alert (`QueuePayload::TaskNotify`) |
| `QueueKind::CameraRequest` | `"camera_request"` | 2 minutes (`120,000 ms`) | Interactive remote camera capture request (`QueuePayload::CameraRequest`) |

### Queue States (`QueueStatus`)

```text
           enqueue_item()
                 │
                 ▼
            [ waiting ] ◄─────────────── retry_send_queue_item()
             │       │                   or reset_interrupted_sending()
  TTL passed │       │ peer online &
             │       │ next_retry_ms <= now
             │       ▼
             │  [ sending ]
             │   │       │
             │   │ ok    │ transport drop / integrity error / max attempts
             │   ▼       ▼
             │ (deleted) [ failed ]
             ▼
         [ expired ]
```

- Successfully delivered items are deleted from `send_queue` (`complete_send_queue_item`) and recorded in `timeline` as completed transfers.
- Failed or expired items remain visible in the UI (`failed` / `expired`) so the user can inspect the error reason, click **Retry**, or remove them; completed/failed/expired history is capped at `MAX_QUEUE_HISTORY_ITEMS = 100`.

### Ordering, Replacement, and Capacity Guarantees

1. **Strict Per-Peer FIFO Sequencing**: Every item receives a monotonically increasing `seq` number (`idx_send_queue_peer_fifo` on `(peer, status, seq ASC)`). Items for a given peer are drained strictly in `seq` order, while queues for different peers drain independently in parallel.
2. **In-Place Clipboard Replacement**: If a peer already has a `waiting` item with `kind IN ('clipboard', 'clipboard_image')`, copying a new text or image clip replaces the existing row in place and preserves its `seq` slot instead of piling up stale clipboard states. Previously staged temporary clipboard image files are cleaned up before replacement.
3. **Capacity Limits**:
   - Maximum active (`waiting` + `sending`) items across the queue: `MAX_QUEUE_ACTIVE_ITEMS = 200`.
   - Maximum active file bytes across the queue: `MAX_QUEUE_ACTIVE_BYTES = 10 GiB` (`10 * 1024 * 1024 * 1024` bytes).
   - Maximum queued clipboard image size: `MAX_QUEUE_CLIPBOARD_IMAGE_BYTES = 50 MiB` in `store.rs` (`clip::MAX_IMAGE_BYTES = 32 MiB` on the wire).
4. **Pre-Send File Integrity Verification (`QueuedFileSpec::verify_integrity`)**: When a file is enqueued, `Store` records its `size` and last-modified timestamp (`mtime_ms`). Immediately before opening the QUIC stream, `verify_integrity` re-stats the path; if the file was deleted, truncated, or modified after queueing, the item transitions to `QueueStatus::Failed` with a descriptive error rather than streaming corrupt or unintended data.
5. **Ephemeral File Descriptor Staging (`stage_file_for_queue`)**: On Android, files shared from `content://` URIs may be backed by transient `ContentResolver` file descriptors that close when the share activity finishes. `Node` copies transient inputs into `<data_dir>/queue_staging/<queue_id>/<idx>_<sanitized_name>` (`staged: true`) and deletes the staging directory (`cleanup_staging_id`) as soon as the item completes, is canceled, or is pruned.
6. **Crash Recovery and Zero-Polling Coordinator**:
   - On startup, `Store::reset_interrupted_sending` resets any item left in `'sending'` state by a previous process crash back to `'waiting'` (`next_retry_ms = 0`).
   - The background queue coordinator (`node.rs`) never busy-polls SQLite: it computes the exact next wakeup deadline (`Store::next_queue_wake_ms`) across pending retry timers and TTL expirations, and awaits `std::future::pending()` when the queue is empty until woken by `queue_notify` (triggered on enqueue, retry, or peer session connection).

### Sealed Queue Content (`QueueSeal`)

Clipboard text and images, links and task alerts in `send_queue` are sealed with the platform `KeyProtector` before they reach SQLite: `payload_json` holds `sealed1:` and base64, `payload_blob` holds sealed bytes, and `title` and `detail` hold a plain label, with the real ones worked out when the row is read. Rows from older versions are sealed at startup, followed by `VACUUM`, and `secure_delete` is on. These kinds are deleted when they expire rather than kept as `expired`. Clearing clipboard history removes waiting clips, and clearing everything also removes waiting links and alerts. File items keep their names and paths in the clear, and stay for Retry.

## Encrypted Clipboard History (`clipboard_history.rs`)

Recent clipboard items are persisted in `<data_dir>/clipboard_history.enc` and `<data_dir>/clipboard_images/<id>.enc`:

- **At-Rest Protection (`KeyProtector`)**: Both the CBOR index (`clipboard_history.enc`, prefixed with 5-byte magic header `NLCB\x01`) and each image file (`clipboard_images/<id>.enc`, prefixed with 5-byte magic header `NLCI\x01`) are sealed directly via `KeyProtector::seal` and `KeyProtector::open` (`SecretServiceProtector` using AES-256-GCM under a key held by the desktop's Secret Service on Linux, or `KeystoreKeyProtector` using AES-256-GCM inside Android Keystore on Android). With no keyring on Linux they are stored unsealed; see the [security model](security-model.md#cryptographic-device-identity).
- **Atomic Writes**: Updates write to `.tmp` files (`clipboard_history.enc.tmp` and `<id>.enc.tmp`) and atomically rename them into place.
- **Retention Limits**:
  - Up to `MAX_ENTRIES = 50` entries.
  - Automatic 7-day expiration (`RETENTION_SECS = 7 * 24 * 60 * 60`).
  - Maximum single image size: `MAX_IMAGE_BYTES = 16 MiB`; maximum total image vault size: `MAX_TOTAL_IMAGE_BYTES = 128 MiB`.
- **Sensitive Content Filtering**: Clips flagged as sensitive (`ClipMeta::is_sensitive`, such as password manager entries with `ClipboardDescription` / `ExcludeClipboardContentFromMonitorProcessing`) or one-time passcodes (OTPs) are excluded from persistent history.
