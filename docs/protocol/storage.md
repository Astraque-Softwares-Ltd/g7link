# Storage service (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md) and the
> [files service](files.md). License: CC BY 4.0.

Lets a PC browse and mount a phone's shared storage in its file manager (a
FUSE mount at `~/G7Link/<Phone name>` on Linux) or inspect it from the CLI:
folders list on demand, files are read in ranges as they are opened, files dropped in upload with resume, and
renames, folder creation and deletions round-trip to the phone.

## 1. Capabilities

| ID | Offered by | Meaning |
|---|---|---|
| `storage.read` | phone | Lists folders and reads file byte ranges in the phone's shared storage (or folders picked with the Storage Access Framework) |
| `storage.write` | phone | Creates folders, writes/uploads files, renames and deletes entries in the phone's shared storage (or picked SAF folders) |
| `storage.mount` | PC | Mounts the phone's shared storage in the file manager |

Access is guarded by the `storage` per-device toggle (**off by default** on the
phone, and configurable per device on both sides). While `storage` is off on
the phone for a PC, the phone rejects every `storage.*` request with `DENIED`
and prompts the user once so they can allow that PC.

## 2. Relative paths

All paths in `storage.*` messages are UTF-8, `/`-separated, and relative to the
phone's shared storage root:

- `""` (empty string) names the storage root itself (valid in `storage.list`
  and `storage.changed`).
- Non-root paths (`"DCIM/Camera"`, `"Download/report.pdf"`) consist of 1 to 32
  `/`-separated segments, at most 1 024 UTF-8 bytes in total.
- Each segment is 1 to 255 UTF-8 bytes, never `"."` or `".."`, and contains no
  `/`, `\`, NUL (`\0`) or control characters.
- Leading or trailing `/` is rejected.
- The phone resolves paths strictly inside its shared root (rejecting symlinks
  pointing outside the canonical root) and never exposes `/data` or the app's
  private directories.
- With full storage access, the root also lists each mounted SD card or USB
  drive as a folder, named as the phone names it (`"SD card"`, numbered if a
  folder in internal storage has that name). Paths under it are on that
  volume, with the same rules; the folder itself can't be renamed or deleted,
  and a rename between volumes is refused (`FAILED`), so the PC copies instead.

## 3. Requests and events

The requests are RPCs (PC → phone, [protocol v1](v1.md) §4.3);
`storage.changed` goes one way on the `storage` lane (phone → PC).

```
RPC storage/list     t = "storage.list"     b = { path: text, offset?: uint }
  answer (paged): { entries: [ StorageEntry, ... ], more?: bool }
RPC storage/mkdir    t = "storage.mkdir"    b = { path: text }                       answer: ok
RPC storage/rename   t = "storage.rename"   b = { from: text, to: text }             answer: ok
RPC storage/delete   t = "storage.delete"   b = { path: text, confirmed?: bool }     answer: ok

t = "storage.changed"   b = { path: text }                                          // storage lane
```

```
StorageEntry = {
  name:     text,        // Entry name (single path segment, 1 to 255 bytes)
  size:     uint,        // Bytes (0 for directories)
  modified: int,         // Last-modified timestamp (Unix milliseconds)
  is_dir:   bool,        // True for directories, false for regular files
}
```

### 3.1 `storage.list`

Lists the immediate children of `path` (`""` for the root). Symlinks pointing
outside the shared root and unreadable/private entries are omitted. Entries come
directories first, then by name, ignoring case.

A folder whose entries don't fit one frame comes in parts of one answer
([protocol v1](v1.md) §4.4). `offset` (default 0) skips that many entries;
`more: true`, in any part, says entries follow that this answer left out, and
the requester asks again with `offset` past the ones it has, until `more` is
absent or false.

Errors: `NOT_FOUND` if `path` does not exist or is not a directory; `DENIED` if
`storage` is off or permission is not granted; `INVALID` if `path` is malformed.

### 3.2 `storage.mkdir`

Creates the directory `path` (and any missing parents inside the shared root).
Succeeds if `path` already exists as a directory.

### 3.3 `storage.rename`

Renames or moves `from` to `to` within the shared storage root.

### 3.4 `storage.delete`

Deletes `path`. On Android 11+, media items indexed by `MediaStore` are moved
to the system trash when possible; otherwise, deletion requires `confirmed =
true` (indicating the user confirmed deletion on the PC, e.g. in the file manager
or the CLI).

### 3.5 `storage.changed`

Sent (debounced) by the phone to connected PCs that have listed `path` when
files or subfolders inside `path` change on the phone. Not answered.

## 4. Stream operations (`svc = "storage"`, `v = 1`)

File reads and writes use dedicated bidirectional QUIC streams opened by the PC
([protocol v1](v1.md) §4.7), so large transfers and ranged hydrations never
hold up anything else.

### 4.1 `storage.read` (`op = "read"`)

```
PC -> phone:  t = "stream"             b = { svc: "storage", op: "read", v: 1 }
PC -> phone:  t = "storage.read" id=1  b = { path: text, offset: uint, length?: uint }
phone -> PC:  t = "storage.read.meta" re=1  b = { size: uint, modified: int, length: uint }
phone -> PC:  <length raw bytes, then FIN>
```

Reads `length` bytes (or until EOF when `length` is omitted) starting at byte
`offset`. The phone replies with `storage.read.meta` giving the total file
`size`, `modified` time (Unix ms), and the exact `length` of raw bytes that
follow on the stream, or an `error` frame (`NOT_FOUND`, `DENIED`, `INVALID`)
before any raw bytes.

### 4.2 `storage.write` (`op = "write"`)

```
PC -> phone:  t = "stream"              b = { svc: "storage", op: "write", v: 1 }
PC -> phone:  t = "storage.write" id=1  b = { id: text, path: text, size: uint, modified?: int }
phone -> PC:  t = "storage.write.accept" re=1  b = { have: uint }
PC -> phone:  <size - have raw bytes starting at offset have, then FIN>
phone -> PC:  t = "storage.write.done"         b = { size: uint, modified: int }
```

Uploads a file of `size` bytes to `path`. Like `files` ([files](files.md) §3),
writes are staged by `(peer, id, path, size)` so an interrupted upload retried
with the same `id` resumes from `have` bytes. Once all `size` bytes arrive, the
phone atomically commits the file to `path` (applying `modified` if given) and
sends `storage.write.done`.

## 5. Security and privacy

- The phone never exposes `/data` or its own private app directories.
- Every path is validated before touching the filesystem, and canonical paths
  are checked to stay inside the shared storage root (or within the user's
  chosen SAF trees when full storage access is not granted).
- Paths and file contents are never logged ([protocol v1](v1.md) §11).
