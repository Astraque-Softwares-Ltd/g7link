# Photos service (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md) and the
> [files service](files.md). License: CC BY 4.0.

A phone tells its PCs about each photo or screenshot it takes, with a small
preview, so the PC can offer it right away, and lets a PC browse its albums,
photos and videos with thumbnails fetched on demand. Full files come only when
the PC asks, as an ordinary files transfer.

## 1. Capabilities

| ID | Offered by | Meaning |
|---|---|---|
| `photos.read` | phone | Announces new photos, lists albums and items, serves thumbnails, and sends full files on request (needs the phone's photos permission) |
| `photos.show` | PC | Shows announced photos and the phone's gallery |

A phone announces photos and library changes only to PCs that offer
`photos.show`, and only while both devices allow it (the `photos` device
toggle, on by default, on each side).

## 2. Messages

`photos.new` and `photos.changed` go one way on the `photos` lane (phone →
PC). The rest are RPCs (PC → phone, [protocol v1](v1.md) §4.3); the lists are
paged (§4.4 there).

```
t = "photos.new"                     b = {
  id:         text,      // The phone's ID for it: 1 to 64 printable ASCII characters
  name:       text,      // A file name, as in files.offer
  size:       uint,      // Bytes
  taken:      int,       // Unix seconds
  screenshot: bool,      // Optional, false when missing
  thumb:      bytes,     // A JPEG preview: at most 96 KiB, at most 512 pixels a side
}
t = "photos.changed"                 b = {}

RPC photos/albums   t = "photos.albums"   b = {}
  answer (paged): { albums: [ PhotoAlbum, ... ] }
RPC photos/list     t = "photos.list"     b = { album?: text, before?: int, before_id?: text, limit: uint }
  answer (paged): { items: [ PhotoItem, ... ] }
RPC photos/thumbs   t = "photos.thumbs"   b = { ids: [ text, ... ] }
  answer (paged): { thumbs: [ PhotoThumb, ... ] }
RPC photos/get      t = "photos.get"      b = { id?: text, ids?: [ text, ... ] }
  answer: ok { transfer: text }
RPC photos/delete   t = "photos.delete"   b = { ids: [ text, ... ] }
  answer: ok { count: uint }
```

```
PhotoAlbum = {
  id:     text,          // Album bucket ID on the phone
  name:   text,          // Display name, e.g. "Camera", "Screenshots"
  count:  uint,          // Number of photos and videos in the album
  cover?: text,          // Newest item's ID, for its thumbnail
}

PhotoItem = {
  id:        text,       // Item ID on the phone (1 to 64 printable ASCII characters)
  name:      text,       // File name, sanitized as in files.offer
  date:      int,        // When it was taken (Unix ms), or else when it was saved
  size:      uint,       // Full file size in bytes
  width:     uint,       // Pixels (0 when unknown)
  height:    uint,       // Pixels (0 when unknown)
  duration?: uint,       // Video duration in milliseconds (absent for photos)
  album?:    text,       // Album bucket ID
}

PhotoThumb = {
  id:   text,            // Item ID
  data: bytes,           // A JPEG thumbnail: at most 96 KiB, ~256 pixels a side
}
```

### 2.1 `photos.new`

Sent once per new photo, to the PCs connected at the time; a PC that
connects later doesn't hear about it. It isn't answered. Phones **SHOULD**
leave out bursts (a camera's burst mode, a downloaded album) beyond the
latest few. A PC drops announcements that break the rules above.

### 2.2 `photos.albums`

Lists the phone's photo and video albums (MediaStore buckets), with each
album's item count and newest item ID (`cover`).

### 2.3 `photos.list`

Lists photos and videos on the phone (or in `album` when given), newest
first by `date`, then in an order of the phone's own that never changes
(Android: by media ID). To get the next page, the PC sends the last item of
the one before as `before` (its `date`) and `before_id` (its `id`); the
phone answers with the items after it in that order, so items sharing a
date are neither skipped nor repeated. With `before` alone, only items
older than `before` come. `limit` is capped at 200; a page larger than a
frame comes in parts. Fewer items than `limit` means there are no more.

### 2.4 `photos.thumbs`

Fetches small JPEG thumbnails for up to 24 item IDs in one batch. Items
whose thumbnail cannot be generated are omitted from `thumbs` rather than
failing the batch. The PC caches thumbnails on disk and only asks for
items currently visible in the grid.

### 2.5 `photos.get`

Asks for one (`id`) or several (`ids`, up to 200) photos or videos. The
phone starts sending them as a files transfer ([files](files.md) §2) and
answers with that transfer's `id`; the PC finds the file(s) there when the
transfer is done.

Errors: `NOT_FOUND` when a requested item is gone; `DENIED` when photos or
files are off for the PC.

### 2.6 `photos.changed`

Sent (debounced) when the phone's photo or video library changes so
connected PCs can refresh their album and item lists. It isn't answered.

### 2.7 `photos.delete`

PC to phone: move these items to the phone's trash, where they can still be
restored. `ids` holds up to 24 items, so a PC sends a larger selection in batches. The phone answers `ok` with
`count`, how many went. Items it couldn't move stay and aren't counted. Moving
without a prompt needs Android 11 or newer and All files access, so otherwise
`count` is 0.

Errors: `DENIED` when photos are off for the PC, `INTERNAL` when the phone couldn't delete them.

## 3. Privacy

- Nothing is sent before the user gives the phone app access to photos and
  videos (including Android 14's "Select photos" access, where only the
  chosen items are visible), and only new photos taken from then on are
  announced with `photos.new`.
- Previews, thumbnails and photos are never logged; names neither ([protocol v1](v1.md) §11).

