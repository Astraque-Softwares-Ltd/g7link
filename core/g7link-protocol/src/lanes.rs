// SPDX-License-Identifier: MPL-2.0
//! Lanes (`docs/protocol/v1.md` §4): which stream each message type travels
//! on, the RPC operations, and the paging of their list answers. No I/O.
//!
//! Control carries session housekeeping. Each service has a lane per
//! direction for its events and the requests whose order matters. Every
//! other request is an RPC on a stream of its own: the opener sends the
//! header `{svc, op, v}` and one request frame `t = "<svc>.<op>"` without an
//! `id`, and the responder answers with `part` frames and then one `ok` or
//! `error`. A list answer is split into parts of about [`RPC_PART_TARGET`]
//! bytes ([`Paged`]).

use serde::Serialize;

use crate::messages::{
    AgentBranch, AgentBranches, AgentCommand, AgentCommands, AgentFileHit, AgentFiles, AgentFound,
    AgentFoundItems, AgentFoundList, AgentGitReview, AgentGitReviewFile, AgentHit, AgentHits, AgentItem,
    AgentItems, AgentPrRow, AgentPrRows, AgentSession, AgentSessionItems, AgentSessions, AgentThread,
    AgentThreads, AgentTurn, AgentTurnFile, AgentTurnFiles, AgentTurns, CallLog, CallLogEntry, Contact,
    ContactsList, PhoneApp, PhoneApps, PhotoAlbum, PhotoAlbumsList, PhotoItem, PhotoItems, PhotoThumb,
    PhotoThumbsList, SmsMessage, SmsMessages, SmsThread, SmsThreads, StorageEntries, StorageEntry, clip,
    continuity_camera, files, types,
};

/// The `op` of a lane's stream header.
pub const OP_LANE: &str = "lane";
/// The `v` of every lane and RPC stream header.
pub const VERSION: u32 = 1;
/// A paged answer is split so each `part`, and the `ok`, encodes to at most
/// this many bytes. An item larger than that goes alone in its frame, still
/// under [`crate::MAX_FRAME_LEN`].
pub const RPC_PART_TARGET: usize = 256 * 1024;

/// Service names: a lane's or an RPC's `svc`, its messages' prefix and its
/// capabilities' prefix.
pub mod svc {
    pub const NOTIFY: &str = "notify";
    pub const CLIP: &str = "clip";
    pub const MEDIA: &str = "media";
    pub const CALL: &str = "call";
    pub const CONTACTS: &str = "contacts";
    pub const SMS: &str = "sms";
    pub const PHOTOS: &str = "photos";
    pub const STORAGE: &str = "storage";
    pub const FILES: &str = "files";
    pub const PC: &str = "pc";
    pub const TASK: &str = "task";
    pub const TOGGLES: &str = "toggles";
    pub const DECK: &str = "deck";
    pub const INPUT: &str = "input";
    pub const MIRROR: &str = "mirror";
    pub const WEBCAM: &str = "webcam";
    pub const MIC: &str = "mic";
    pub const CAMERA: &str = "camera";
    pub const AGENT: &str = "agent";
    pub const LINK: &str = "link";
    pub const ELEVATED: &str = "elevated";
}

/// Services with a lane: each side opens at most one for each, the first
/// time it has something for it.
pub const LANE_SERVICES: &[&str] = &[
    svc::NOTIFY,
    svc::CLIP,
    svc::MEDIA,
    svc::CALL,
    svc::CONTACTS,
    svc::SMS,
    svc::PHOTOS,
    svc::STORAGE,
    svc::FILES,
    svc::PC,
    svc::TASK,
    svc::TOGGLES,
    svc::DECK,
    svc::INPUT,
    svc::MIRROR,
    svc::WEBCAM,
    svc::MIC,
    svc::CAMERA,
    svc::AGENT,
];

/// Every service: those with a lane, and those with only RPCs.
pub const SERVICES: &[&str] = &[
    svc::NOTIFY,
    svc::CLIP,
    svc::MEDIA,
    svc::CALL,
    svc::CONTACTS,
    svc::SMS,
    svc::PHOTOS,
    svc::STORAGE,
    svc::FILES,
    svc::PC,
    svc::TASK,
    svc::TOGGLES,
    svc::DECK,
    svc::INPUT,
    svc::MIRROR,
    svc::WEBCAM,
    svc::MIC,
    svc::CAMERA,
    svc::AGENT,
    svc::LINK,
    svc::ELEVATED,
];

/// Where a message type travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    /// The session's control stream, opened first by the dialer.
    Control,
    /// This service's lane: its events and ordered requests, one lane per
    /// direction, answers coming back on the same stream with `re`.
    Service(&'static str),
    /// The one request frame of an RPC stream whose header is `svc`/`op`.
    Rpc { svc: &'static str, op: &'static str },
    /// A frame inside the media or bulk stream whose header is `svc`/`op`,
    /// after the header. Its contents are the service's own.
    Stream { svc: &'static str, op: &'static str },
    /// `ok` and `error`, on the stream of what they answer; `part`, on RPC
    /// streams only.
    Reply,
    /// `stream`: the first frame of every stream but control.
    Header,
    /// The pairing ceremony (`g7link-pair/0`), never a session.
    Pairing,
    /// Only the Bluetooth channel carries it (`docs/protocol/bluetooth.md` §4.4).
    Bluetooth,
}

/// An RPC operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RpcOp {
    /// The request frame's type: `<svc>.<op>`.
    pub t: &'static str,
    pub svc: &'static str,
    pub op: &'static str,
    /// Its answer is a list that may come in `part`s ([`Paged`]).
    pub paged: bool,
}

const fn rpc(t: &'static str, svc: &'static str, op: &'static str, paged: bool) -> RpcOp {
    RpcOp { t, svc, op, paged }
}

/// Every RPC operation (`docs/protocol/v1.md` §9.3).
pub const RPCS: &[RpcOp] = &[
    rpc(types::NOTIFY_DISMISS, svc::NOTIFY, "dismiss", false),
    rpc(types::NOTIFY_ACTION, svc::NOTIFY, "action", false),
    rpc(types::MEDIA_ICON, svc::MEDIA, "icon", false),
    rpc(types::CALL_LOG, svc::CALL, "log", true),
    rpc(types::CALL_DIAL, svc::CALL, "dial", false),
    rpc(types::CALL_SIMS, svc::CALL, "sims", false),
    rpc(types::CONTACTS_LIST, svc::CONTACTS, "list", true),
    rpc(types::SMS_THREADS, svc::SMS, "threads", true),
    rpc(types::SMS_MESSAGES, svc::SMS, "messages", true),
    rpc(types::SMS_SEND, svc::SMS, "send", false),
    rpc(types::SMS_PART, svc::SMS, "part", false),
    rpc(types::PHOTOS_ALBUMS, svc::PHOTOS, "albums", true),
    rpc(types::PHOTOS_LIST, svc::PHOTOS, "list", true),
    rpc(types::PHOTOS_THUMBS, svc::PHOTOS, "thumbs", true),
    rpc(types::PHOTOS_GET, svc::PHOTOS, "get", false),
    rpc(types::PHOTOS_DELETE, svc::PHOTOS, "delete", false),
    rpc(types::STORAGE_LIST, svc::STORAGE, "list", true),
    rpc(types::STORAGE_MKDIR, svc::STORAGE, "mkdir", false),
    rpc(types::STORAGE_RENAME, svc::STORAGE, "rename", false),
    rpc(types::STORAGE_DELETE, svc::STORAGE, "delete", false),
    rpc(types::PC_POWER, svc::PC, "power", false),
    rpc(types::PC_WAKE_RELAY, svc::PC, "wake.relay", false),
    rpc(types::PC_CAPTURE_SHOT, svc::PC, "capture.shot", false),
    rpc(types::PC_CAPTURE_RECORD_START, svc::PC, "capture.record.start", false),
    rpc(types::PC_CAPTURE_RECORD_STOP, svc::PC, "capture.record.stop", false),
    rpc(types::LINK_OPEN, svc::LINK, "open", false),
    rpc(types::ELEVATED_USB_START, svc::ELEVATED, "usb.start", false),
    rpc(types::INPUT_CHECK, svc::INPUT, "check", false),
    rpc(types::INPUT_APP_ICON, svc::INPUT, "app_icon", false),
    rpc(types::MIRROR_START, svc::MIRROR, "start", false),
    rpc(types::MIRROR_APPS, svc::MIRROR, "apps", true),
    rpc(types::WEBCAM_START, svc::WEBCAM, "start", false),
    rpc(types::MIC_START, svc::MIC, "start", false),
    rpc(types::CAMERA_CAPTURE_REQUEST, svc::CAMERA, "capture.request", false),
    rpc(types::AGENT_THREADS, svc::AGENT, "threads", true),
    rpc(types::AGENT_READ, svc::AGENT, "read", true),
    rpc(types::AGENT_SESSIONS, svc::AGENT, "sessions", true),
    rpc(types::AGENT_SESSION_READ, svc::AGENT, "session.read", true),
    rpc(types::AGENT_FILES, svc::AGENT, "files", true),
    rpc(types::AGENT_COMMANDS, svc::AGENT, "commands", true),
    rpc(types::AGENT_START, svc::AGENT, "start", false),
    rpc(types::AGENT_SEND, svc::AGENT, "send", false),
    rpc(types::AGENT_CANCEL, svc::AGENT, "cancel", false),
    rpc(types::AGENT_ANSWER, svc::AGENT, "answer", false),
    rpc(types::AGENT_CONFIGURE, svc::AGENT, "configure", false),
    rpc(types::AGENT_QUEUE, svc::AGENT, "queue", false),
    rpc(types::AGENT_REPLY, svc::AGENT, "reply", false),
    rpc(types::AGENT_IMPLEMENT, svc::AGENT, "implement", false),
    rpc(types::AGENT_WATCH, svc::AGENT, "watch", false),
    rpc(types::AGENT_SESSION_WATCH, svc::AGENT, "session.watch", false),
    rpc(types::AGENT_SESSION_CONTINUE, svc::AGENT, "session.continue", false),
    rpc(types::AGENT_DRAFT_GET, svc::AGENT, "draft.get", false),
    rpc(types::AGENT_DRAFT_SET, svc::AGENT, "draft.set", false),
    rpc(types::AGENT_TURNS, svc::AGENT, "turns", true),
    rpc(types::AGENT_DIFF_FILES, svc::AGENT, "diff.files", true),
    rpc(types::AGENT_DIFF_PATCH, svc::AGENT, "diff.patch", true),
    rpc(types::AGENT_SEARCH, svc::AGENT, "search", true),
    rpc(types::AGENT_MARK, svc::AGENT, "mark", true),
    rpc(types::AGENT_RENAME, svc::AGENT, "rename", false),
    rpc(types::AGENT_FORK, svc::AGENT, "fork", false),
    rpc(types::AGENT_REWIND_PREVIEW, svc::AGENT, "rewind.preview", false),
    rpc(types::AGENT_REWIND_APPLY, svc::AGENT, "rewind.apply", false),
    rpc(types::AGENT_REWIND_UNDO, svc::AGENT, "rewind.undo", false),
    rpc(types::AGENT_FOUND, svc::AGENT, "found", true),
    rpc(types::AGENT_FOUND_PREVIEW, svc::AGENT, "found.preview", true),
    rpc(types::AGENT_IMPORT, svc::AGENT, "import", false),
    rpc(types::AGENT_VIEWING, svc::AGENT, "viewing", false),
    rpc(types::AGENT_GIT_STATUS, svc::AGENT, "git.status", true),
    rpc(types::AGENT_GIT_PATCH, svc::AGENT, "git.patch", true),
    rpc(types::AGENT_GIT_REVIEW, svc::AGENT, "git.review", true),
    rpc(types::AGENT_GIT_COMMIT, svc::AGENT, "git.commit", false),
    rpc(types::AGENT_GIT_PUSH, svc::AGENT, "git.push", false),
    rpc(types::AGENT_GIT_FETCH, svc::AGENT, "git.fetch", false),
    rpc(types::AGENT_GIT_BRANCHES, svc::AGENT, "git.branches", true),
    rpc(types::AGENT_GIT_SWITCH, svc::AGENT, "git.switch", false),
    rpc(types::AGENT_GIT_JOB, svc::AGENT, "git.job", false),
    rpc(types::AGENT_PR_VIEW, svc::AGENT, "pr.view", false),
    rpc(types::AGENT_PR_CREATE, svc::AGENT, "pr.create", false),
    rpc(types::AGENT_PR_LIST, svc::AGENT, "pr.list", true),
    rpc(types::AGENT_WRITE, svc::AGENT, "write", false),
    rpc(types::AGENT_SESSION_REPLY, svc::AGENT, "session.reply", false),
    rpc(types::AGENT_HANDOFF_OPEN, svc::AGENT, "handoff.open", false),
];

/// The RPC operation whose request frame has type `t`.
pub fn rpc_of(t: &str) -> Option<RpcOp> {
    RPCS.iter().find(|r| r.t == t).copied()
}

/// The RPC operation an incoming stream header names, if it names one.
pub fn rpc_by_header(svc: &str, op: &str) -> Option<RpcOp> {
    RPCS.iter().find(|r| r.svc == svc && r.op == op).copied()
}

/// The lane message type `t` travels on, or `None` for a type this build
/// doesn't know.
pub fn lane_of(t: &str) -> Option<Lane> {
    use types::*;
    if let Some(op) = rpc_of(t) {
        return Some(Lane::Rpc { svc: op.svc, op: op.op });
    }
    let lane = match t {
        HELLO | HELLO_UPDATE | PING | PONG | DEVICE_BATTERY | DEVICE_INFO | DEVICE_UPDATE | DEVICE_RING
        | DEVICE_RING_STOPPED | PAIR_REVOKE | BLE_KEY | BLE_USE => Lane::Control,
        OK | ERROR | PART => Lane::Reply,
        STREAM => Lane::Header,
        PAIR_REQUEST | PAIR_ACCEPT | PAIR_COMMIT | PAIR_NONCE | PAIR_REVEAL | PAIR_CONFIRM | PAIR_DONE => {
            Lane::Pairing
        }
        BLE_OTP => Lane::Bluetooth,

        NOTIFY_SNAPSHOT | NOTIFY_POSTED | NOTIFY_REMOVED | NOTIFY_SYNC | NOTIFY_HOLD => {
            Lane::Service(svc::NOTIFY)
        }
        CLIP_SET | CLIP_CHANGED => Lane::Service(svc::CLIP),
        MEDIA_STATE | MEDIA_SYNC | MEDIA_COMMAND | MEDIA_WATCH | MEDIA_BT => Lane::Service(svc::MEDIA),
        CALL_STATE | CALL_LOG_CHANGED | CALL_ACTION => Lane::Service(svc::CALL),
        CONTACTS_CHANGED => Lane::Service(svc::CONTACTS),
        SMS_CHANGED => Lane::Service(svc::SMS),
        PHOTOS_NEW | PHOTOS_CHANGED => Lane::Service(svc::PHOTOS),
        STORAGE_CHANGED => Lane::Service(svc::STORAGE),
        FILES_RESUME | FILES_CANCEL => Lane::Service(svc::FILES),
        PC_WAKE_INFO | PC_CAPTURE_PROMPT | PC_CAPTURE_RECORD_ENDED => Lane::Service(svc::PC),
        TASK_NOTIFY => Lane::Service(svc::TASK),
        TOGGLES_STATE | TOGGLES_SET => Lane::Service(svc::TOGGLES),
        DECK_LAYOUT | DECK_STATE | DECK_PRESS | DECK_AUDIO_SET => Lane::Service(svc::DECK),
        INPUT_EVENT | INPUT_TEXT_WATCH | INPUT_TEXT_STATE | INPUT_TEXT_FOCUS => Lane::Service(svc::INPUT),
        MIRROR_INPUT | MIRROR_KEYFRAME | MIRROR_RESIZE | MIRROR_SOUND | MIRROR_POWER | MIRROR_NOTICE
        | MIRROR_STOP | MIRROR_CLOCK => Lane::Service(svc::MIRROR),
        WEBCAM_STOP | WEBCAM_KEYFRAME => Lane::Service(svc::WEBCAM),
        MIC_STOP => Lane::Service(svc::MIC),
        CAMERA_CAPTURE_CANCEL => Lane::Service(svc::CAMERA),
        AGENT_CATALOG
        | AGENT_THREAD
        | AGENT_EVENT
        | AGENT_ASK
        | AGENT_ASKED
        | AGENT_QUESTION
        | AGENT_QUESTIONED
        | AGENT_SESSION
        | AGENT_SESSION_ITEM
        | AGENT_DRAFT
        | AGENT_TURN
        | AGENT_CUT
        | AGENT_IMPORT_PROGRESS
        | AGENT_PRESENCE
        | AGENT_GIT
        | AGENT_GIT_PROGRESS
        | AGENT_HANDOFF
        | AGENT_BESIDE => Lane::Service(svc::AGENT),

        MEDIA_FILE | MEDIA_FILE_META => Lane::Stream { svc: svc::MEDIA, op: "file" },
        MEDIA_ART | MEDIA_ART_META => Lane::Stream { svc: svc::MEDIA, op: "art" },
        STORAGE_READ | STORAGE_READ_META => Lane::Stream { svc: svc::STORAGE, op: "read" },
        STORAGE_WRITE | STORAGE_WRITE_ACCEPT | STORAGE_WRITE_DONE => {
            Lane::Stream { svc: svc::STORAGE, op: "write" }
        }
        AGENT_EXPORT | AGENT_EXPORT_META => Lane::Stream { svc: svc::AGENT, op: "export" },
        AGENT_IMAGE_META | AGENT_IMAGE => Lane::Stream { svc: svc::AGENT, op: "image" },
        AGENT_IMAGE_THUMB | AGENT_IMAGE_THUMB_META => Lane::Stream { svc: svc::AGENT, op: "thumb" },
        files::OFFER | files::ACCEPT | files::DONE => {
            Lane::Stream { svc: files::SERVICE, op: files::OP_SEND }
        }
        clip::IMAGE => Lane::Stream { svc: clip::SERVICE, op: clip::OP_IMAGE },
        continuity_camera::RESULT_META => {
            Lane::Stream { svc: continuity_camera::SERVICE, op: continuity_camera::OP_RESULT }
        }
        _ => return None,
    };
    Some(lane)
}

// ---- Paging ----

/// Bytes an encoded part may spend besides its body's fields and items: the
/// envelope (`t`, `b`), the frame's length prefix and the list's own length.
const PART_OVERHEAD: usize = 32;

struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// How many bytes `value` takes in CBOR, without keeping them. Something
/// that can't be encoded counts as `usize::MAX`, so it goes alone in a part,
/// where encoding the part then fails.
pub fn encoded_len<T: Serialize + ?Sized>(value: &T) -> usize {
    let mut counter = Counter(0);
    match ciborium::into_writer(value, &mut counter) {
        Ok(()) => counter.0,
        Err(_) => usize::MAX,
    }
}

/// Groups a list's items into parts as they come, without I/O. Each part
/// holds what fits in the target beside the answer's other fields; an item
/// larger than that goes alone.
#[derive(Debug)]
pub struct PartBuilder<T> {
    room: usize,
    used: usize,
    items: Vec<T>,
}

impl<T: Serialize> PartBuilder<T> {
    /// For parts of about `target` bytes whose other fields take `shell`
    /// bytes ([`encoded_len`] of the body with its list empty).
    pub fn new(target: usize, shell: usize) -> Self {
        PartBuilder {
            room: target.saturating_sub(shell.saturating_add(PART_OVERHEAD)),
            used: 0,
            items: Vec::new(),
        }
    }

    /// Adds `item`. When it doesn't fit beside the items held, those come
    /// back first, as a full part, and `item` starts the next.
    pub fn push(&mut self, item: T) -> Option<Vec<T>> {
        let len = encoded_len(&item);
        let full = (!self.items.is_empty() && self.used.saturating_add(len) > self.room).then(|| {
            self.used = 0;
            std::mem::take(&mut self.items)
        });
        self.used = self.used.saturating_add(len);
        self.items.push(item);
        full
    }

    /// Whether no item is held.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The items of the last part (none for an empty list).
    pub fn finish(self) -> Vec<T> {
        self.items
    }
}

/// A list answer that may come as `part` frames before its `ok`. Every part,
/// and the `ok`, is a whole body of the same type. Its list is split across
/// them in order; the fields every part repeats are the same in each; what
/// only the first carries (an agent page's asks and questions) is empty in
/// the rest. A "more" flag is set if any part sets it.
pub trait Paged: Serialize + Sized {
    type Item: Serialize;

    /// Takes the list out, leaving the rest of the body.
    fn take_items(&mut self) -> Vec<Self::Item>;

    /// Puts `items` in as the list.
    fn set_items(&mut self, items: Vec<Self::Item>);

    /// The body of a part after the first: the fields every part repeats,
    /// and `items`.
    fn continued(&self, items: Vec<Self::Item>) -> Self;

    /// Adds the next part: its items after these, what else it carries.
    fn append(&mut self, more: Self);

    /// This answer as the bodies of its parts, in order, each encoding to
    /// about `target` bytes at most; the last goes as the `ok`. Always at
    /// least one: an empty list is one `ok`.
    fn split(mut self, target: usize) -> Vec<Self> {
        let items = self.take_items();
        let mut builder = PartBuilder::new(target, encoded_len(&self));
        let mut chunks = Vec::new();
        for item in items {
            chunks.extend(builder.push(item));
        }
        chunks.push(builder.finish());
        let mut chunks = chunks.into_iter();
        let first = chunks.next().unwrap_or_default();
        let mut parts = Vec::with_capacity(chunks.len() + 1);
        let rest: Vec<Self> = chunks.map(|items| self.continued(items)).collect();
        self.set_items(first);
        parts.push(self);
        parts.extend(rest);
        parts
    }
}

/// [`Paged`] for an answer that is its list, and fields every part repeats.
macro_rules! paged {
    ($body:ty, $item:ty, $list:ident $(, keep: $keep:ident)* $(, flag: $flag:ident)*) => {
        impl Paged for $body {
            type Item = $item;

            fn take_items(&mut self) -> Vec<$item> {
                std::mem::take(&mut self.$list)
            }

            fn set_items(&mut self, items: Vec<$item>) {
                self.$list = items;
            }

            fn continued(&self, items: Vec<$item>) -> Self {
                Self { $list: items, $($keep: self.$keep.clone(),)* $($flag: self.$flag,)* }
            }

            fn append(&mut self, more: Self) {
                self.$list.extend(more.$list);
                $(self.$flag |= more.$flag;)*
            }
        }
    };
}

paged!(CallLog, CallLogEntry, entries);
paged!(ContactsList, Contact, contacts);
paged!(SmsThreads, SmsThread, threads, keep: app);
paged!(SmsMessages, SmsMessage, messages);
paged!(PhotoAlbumsList, PhotoAlbum, albums);
paged!(PhotoItems, PhotoItem, items);
paged!(PhotoThumbsList, PhotoThumb, thumbs);
paged!(StorageEntries, StorageEntry, entries, flag: more);
paged!(PhoneApps, PhoneApp, apps);
paged!(AgentThreads, AgentThread, threads);
paged!(AgentSessions, AgentSession, sessions);
paged!(AgentSessionItems, AgentItem, items, keep: session, flag: more_before);
paged!(AgentFiles, AgentFileHit, files, flag: more);
paged!(AgentCommands, AgentCommand, commands);
paged!(AgentTurns, AgentTurn, turns);
paged!(AgentTurnFiles, AgentTurnFile, files, keep: adds, keep: dels, flag: more);
paged!(AgentHits, AgentHit, hits, keep: next, flag: titles_only);
paged!(AgentFoundList, AgentFound, found, keep: next_ms, keep: total, flag: scanning, flag: partial);
paged!(AgentFoundItems, AgentItem, items);
paged!(AgentGitReview, AgentGitReviewFile, files, keep: review);
paged!(AgentBranches, AgentBranch, branches, keep: current, flag: more);
paged!(AgentPrRows, AgentPrRow, prs, keep: gh, flag: more);

impl Paged for AgentItems {
    type Item = AgentItem;

    fn take_items(&mut self) -> Vec<AgentItem> {
        std::mem::take(&mut self.items)
    }

    fn set_items(&mut self, items: Vec<AgentItem>) {
        self.items = items;
    }

    /// The thread and `more_before` repeat; the asks and questions go in the
    /// first part only.
    fn continued(&self, items: Vec<AgentItem>) -> Self {
        AgentItems {
            thread: self.thread.clone(),
            items,
            more_before: self.more_before,
            asks: Vec::new(),
            questions: Vec::new(),
        }
    }

    fn append(&mut self, more: Self) {
        self.items.extend(more.items);
        self.asks.extend(more.asks);
        self.questions.extend(more.questions);
        self.more_before |= more.more_before;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Envelope, MAX_FRAME_LEN,
        messages::{
            AgentAsk, AgentQuestion, AgentThread, ContactNumber, PhotoThumb, agent_export, agent_image,
            input, media_art, media_sound, media_watch, mic, mirror, storage, webcam,
        },
    };

    /// Every `pub const … : &str` in the `types` module, read from the source,
    /// so a type added without a lane fails here.
    fn every_type() -> Vec<String> {
        let source = include_str!("messages.rs");
        let start = source.find("\npub mod types {").expect("the types module");
        let end = start + source[start..].find("\n}\n").expect("its end");
        let types: Vec<String> = source[start..end]
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub const "))
            .map(|rest| {
                let value = rest.split('"').nth(1).expect("a string value");
                value.to_owned()
            })
            .collect();
        assert!(types.len() > 100, "found only {} types", types.len());
        types
    }

    #[test]
    fn every_type_has_exactly_one_lane() {
        let types = every_type();
        let mut seen = std::collections::HashSet::new();
        for t in &types {
            assert!(seen.insert(t.as_str()), "{t} is defined twice");
            assert!(lane_of(t).is_some(), "{t} has no lane");
        }
        // Types defined beside their streams also have one.
        for t in [files::OFFER, files::ACCEPT, files::DONE, files::RESUME, files::CANCEL, clip::IMAGE] {
            assert!(lane_of(t).is_some(), "{t}");
        }
        assert_eq!(lane_of("remote.input"), None);
        assert_eq!(lane_of("audio.pcm"), None);
        assert_eq!(lane_of(""), None);
    }

    #[test]
    fn lanes_follow_the_spec_table() {
        use types::*;
        let control = [
            HELLO,
            HELLO_UPDATE,
            PING,
            PONG,
            DEVICE_BATTERY,
            DEVICE_INFO,
            DEVICE_UPDATE,
            DEVICE_RING,
            DEVICE_RING_STOPPED,
            PAIR_REVOKE,
            BLE_KEY,
            BLE_USE,
        ];
        for t in control {
            assert_eq!(lane_of(t), Some(Lane::Control), "{t}");
        }
        for t in [OK, ERROR, PART] {
            assert_eq!(lane_of(t), Some(Lane::Reply), "{t}");
        }
        assert_eq!(lane_of(STREAM), Some(Lane::Header));
        assert_eq!(lane_of(PAIR_REQUEST), Some(Lane::Pairing));
        assert_eq!(lane_of(BLE_OTP), Some(Lane::Bluetooth));
        let lanes = [
            (MEDIA_COMMAND, svc::MEDIA),
            (MEDIA_WATCH, svc::MEDIA),
            (MEDIA_BT, svc::MEDIA),
            (CALL_ACTION, svc::CALL),
            (CALL_LOG_CHANGED, svc::CALL),
            (INPUT_EVENT, svc::INPUT),
            (INPUT_TEXT_STATE, svc::INPUT),
            (TOGGLES_SET, svc::TOGGLES),
            (TOGGLES_STATE, svc::TOGGLES),
            (DECK_AUDIO_SET, svc::DECK),
            (PC_WAKE_INFO, svc::PC),
            (TASK_NOTIFY, svc::TASK),
            (MIRROR_STOP, svc::MIRROR),
            (MIRROR_CLOCK, svc::MIRROR),
            (WEBCAM_KEYFRAME, svc::WEBCAM),
            (CAMERA_CAPTURE_CANCEL, svc::CAMERA),
            (AGENT_SESSION_ITEM, svc::AGENT),
            (AGENT_DRAFT, svc::AGENT),
            (AGENT_TURN, svc::AGENT),
            (AGENT_CUT, svc::AGENT),
            (AGENT_IMPORT_PROGRESS, svc::AGENT),
            (AGENT_PRESENCE, svc::AGENT),
            (AGENT_GIT, svc::AGENT),
            (AGENT_GIT_PROGRESS, svc::AGENT),
            (AGENT_HANDOFF, svc::AGENT),
            (AGENT_BESIDE, svc::AGENT),
        ];
        for (t, service) in lanes {
            assert_eq!(lane_of(t), Some(Lane::Service(service)), "{t}");
        }
        assert_eq!(lane_of(INPUT_CHECK), Some(Lane::Rpc { svc: svc::INPUT, op: "check" }));
        assert_eq!(lane_of(PC_WAKE_RELAY), Some(Lane::Rpc { svc: svc::PC, op: "wake.relay" }));
        assert_eq!(lane_of(AGENT_SESSION_READ), Some(Lane::Rpc { svc: svc::AGENT, op: "session.read" }));
        assert_eq!(lane_of(AGENT_IMPORT), Some(Lane::Rpc { svc: svc::AGENT, op: "import" }));
        assert_eq!(lane_of(AGENT_FOUND_PREVIEW), Some(Lane::Rpc { svc: svc::AGENT, op: "found.preview" }));
        assert_eq!(lane_of(AGENT_REWIND_APPLY), Some(Lane::Rpc { svc: svc::AGENT, op: "rewind.apply" }));
        assert_eq!(lane_of(AGENT_VIEWING), Some(Lane::Rpc { svc: svc::AGENT, op: "viewing" }));
        assert_eq!(lane_of(AGENT_GIT_JOB), Some(Lane::Rpc { svc: svc::AGENT, op: "git.job" }));
        assert_eq!(lane_of(AGENT_WRITE), Some(Lane::Rpc { svc: svc::AGENT, op: "write" }));
        assert_eq!(lane_of(AGENT_SESSION_REPLY), Some(Lane::Rpc { svc: svc::AGENT, op: "session.reply" }));
        assert_eq!(lane_of(AGENT_HANDOFF_OPEN), Some(Lane::Rpc { svc: svc::AGENT, op: "handoff.open" }));
        assert_eq!(lane_of(AGENT_IMAGE), Some(Lane::Stream { svc: svc::AGENT, op: agent_image::OP_UPLOAD }));
        assert_eq!(
            lane_of(AGENT_IMAGE_THUMB),
            Some(Lane::Stream { svc: svc::AGENT, op: agent_image::OP_THUMB })
        );
        assert_eq!(lane_of(AGENT_EXPORT), Some(Lane::Stream { svc: svc::AGENT, op: agent_export::OP }));
        assert_eq!(lane_of(MEDIA_FILE), Some(Lane::Stream { svc: svc::MEDIA, op: media_watch::OP_FILE }));
        assert_eq!(lane_of(MEDIA_ART_META), Some(Lane::Stream { svc: svc::MEDIA, op: media_art::OP }));
        assert_eq!(lane_of(STORAGE_READ), Some(Lane::Stream { svc: svc::STORAGE, op: storage::OP_READ }));
        assert_eq!(
            lane_of(STORAGE_WRITE_DONE),
            Some(Lane::Stream { svc: svc::STORAGE, op: storage::OP_WRITE })
        );
    }

    #[test]
    fn every_rpc_is_named_after_its_header() {
        let mut seen = std::collections::HashSet::new();
        for op in RPCS {
            assert_eq!(op.t, format!("{}.{}", op.svc, op.op), "{op:?}");
            assert!(SERVICES.contains(&op.svc), "{op:?}");
            assert!(seen.insert((op.svc, op.op)), "{op:?} twice");
            assert_eq!(rpc_of(op.t), Some(*op));
            assert_eq!(rpc_by_header(op.svc, op.op), Some(*op));
            assert_ne!(op.op, OP_LANE);
        }
        assert_eq!(RPCS.len(), 82);
        let paged: Vec<&str> = RPCS.iter().filter(|op| op.paged).map(|op| op.t).collect();
        assert_eq!(
            paged,
            [
                types::CALL_LOG,
                types::CONTACTS_LIST,
                types::SMS_THREADS,
                types::SMS_MESSAGES,
                types::PHOTOS_ALBUMS,
                types::PHOTOS_LIST,
                types::PHOTOS_THUMBS,
                types::STORAGE_LIST,
                types::MIRROR_APPS,
                types::AGENT_THREADS,
                types::AGENT_READ,
                types::AGENT_SESSIONS,
                types::AGENT_SESSION_READ,
                types::AGENT_FILES,
                types::AGENT_COMMANDS,
                types::AGENT_TURNS,
                types::AGENT_DIFF_FILES,
                types::AGENT_DIFF_PATCH,
                types::AGENT_SEARCH,
                types::AGENT_MARK,
                types::AGENT_FOUND,
                types::AGENT_FOUND_PREVIEW,
                types::AGENT_GIT_STATUS,
                types::AGENT_GIT_PATCH,
                types::AGENT_GIT_REVIEW,
                types::AGENT_GIT_BRANCHES,
                types::AGENT_PR_LIST,
            ]
        );
        assert_eq!(rpc_of(types::INPUT_EVENT), None, "a lane message");
        assert_eq!(rpc_by_header(svc::INPUT, OP_LANE), None);
    }

    #[test]
    fn every_lane_message_belongs_to_a_lane_service() {
        for t in every_type() {
            if let Some(Lane::Service(service)) = lane_of(&t) {
                assert!(LANE_SERVICES.contains(&service), "{t}");
                assert!(t.starts_with(&format!("{service}.")), "{t} isn't named after {service}");
            }
        }
        for service in LANE_SERVICES {
            assert!(SERVICES.contains(service));
        }
    }

    #[test]
    fn stream_services_are_the_spec_names() {
        assert_eq!(input::SERVICE, svc::INPUT);
        assert_eq!(input::OP_MOTION, "motion");
        assert_eq!((media_sound::SERVICE, media_sound::OP), (svc::MEDIA, "sound"));
        assert_eq!(media_sound::CAP, "media.sound");
        for service in [
            files::SERVICE,
            clip::SERVICE,
            mirror::SERVICE,
            webcam::SERVICE,
            mic::SERVICE,
            storage::SERVICE,
            continuity_camera::SERVICE,
            agent_export::SERVICE,
            agent_image::SERVICE,
            media_watch::SERVICE,
            media_art::SERVICE,
        ] {
            assert!(SERVICES.contains(&service), "{service}");
        }
        let lane = crate::messages::StreamHeader::lane(svc::CALL);
        assert_eq!((lane.svc.as_str(), lane.op.as_str(), lane.v), ("call", "lane", 1));
        let header = crate::messages::StreamHeader::rpc(svc::AGENT, "read");
        assert_eq!(rpc_by_header(&header.svc, &header.op).map(|op| op.t), Some(types::AGENT_READ));
    }

    fn session(id: &str) -> AgentSession {
        AgentSession {
            id: id.into(),
            agent: "claude".into(),
            folder: "app".into(),
            title: "A session".into(),
            state: crate::messages::agent::session_state::NEEDS_YOU.into(),
            guess: false,
            source: crate::messages::agent::session_source::DESKTOP_APP.into(),
            updated: 1,
            step: None,
            summary: None,
            waiting_for: None,
            can_continue: false,
            blocked: None,
            since: None,
            limit: None,
        }
    }

    /// The bytes of a frame of `body` as message `t`, its length prefix included.
    fn frame_len<T: Serialize>(t: &str, body: &T) -> usize {
        let payload = Envelope::new(t, body).unwrap().to_cbor().len();
        let prefix = (usize::BITS - payload.leading_zeros()).div_ceil(7).max(1) as usize;
        payload + prefix
    }

    /// Splits at `target`, checks each part's size, appends them back
    /// together and returns the result with the number of parts.
    fn round_trip<B: Paged + Clone + PartialEq + std::fmt::Debug>(body: B, target: usize) -> (B, usize) {
        let parts = body.clone().split(target);
        assert!(!parts.is_empty());
        let count = parts.len();
        for (i, part) in parts.iter().enumerate() {
            let t = if i + 1 == count { types::OK } else { types::PART };
            assert!(frame_len(t, part) <= MAX_FRAME_LEN);
        }
        let mut parts = parts.into_iter();
        let mut whole = parts.next().unwrap();
        parts.for_each(|part| whole.append(part));
        assert_eq!(whole, body);
        (whole, count)
    }

    fn contact(i: usize, photo: usize) -> Contact {
        Contact {
            id: format!("c{i}"),
            name: format!("Contact {i}"),
            numbers: vec![ContactNumber { number: format!("+1555{i:07}"), label: None }],
            starred: false,
            photo: (photo > 0).then(|| vec![i as u8; photo]),
        }
    }

    #[test]
    fn a_list_larger_than_a_frame_splits_in_order_under_the_target() {
        // 3 MiB of 16 KiB photos, as two_devices.rs does over the wire.
        let list = ContactsList { contacts: (0..192).map(|i| contact(i, 16 * 1024)).collect() };
        assert!(frame_len(types::OK, &list) > MAX_FRAME_LEN);
        let parts = list.clone().split(RPC_PART_TARGET);
        assert!(parts.len() >= 12, "{} parts", parts.len());
        for part in &parts {
            let len = frame_len(types::PART, part);
            assert!(len <= RPC_PART_TARGET, "{len}");
            assert!(!part.contacts.is_empty());
        }
        let ids: Vec<String> = parts.iter().flat_map(|p| p.contacts.iter().map(|c| c.id.clone())).collect();
        assert_eq!(ids, (0..192).map(|i| format!("c{i}")).collect::<Vec<_>>());
        round_trip(list, RPC_PART_TARGET);
    }

    #[test]
    fn an_empty_list_is_one_ok_and_a_small_one_is_too() {
        let (_, parts) = round_trip(ContactsList { contacts: Vec::new() }, RPC_PART_TARGET);
        assert_eq!(parts, 1);
        let (_, parts) = round_trip(CallLog { entries: Vec::new() }, RPC_PART_TARGET);
        assert_eq!(parts, 1);
        let (_, parts) =
            round_trip(ContactsList { contacts: (0..10).map(|i| contact(i, 0)).collect() }, 4096);
        assert_eq!(parts, 1);
    }

    #[test]
    fn a_single_oversized_item_goes_alone() {
        let contacts = vec![contact(0, 10), contact(1, 300 * 1024), contact(2, 10), contact(3, 10)];
        let list = ContactsList { contacts };
        let parts = list.clone().split(RPC_PART_TARGET);
        let sizes: Vec<usize> = parts.iter().map(|p| p.contacts.len()).collect();
        assert_eq!(sizes, [1, 1, 2]);
        assert!(frame_len(types::PART, &parts[1]) > RPC_PART_TARGET);
        assert!(frame_len(types::PART, &parts[1]) <= MAX_FRAME_LEN);
        round_trip(list, RPC_PART_TARGET);
    }

    #[test]
    fn the_builder_flushes_at_the_boundary() {
        let item = vec![0u8; 100];
        let one = encoded_len(&item);
        // Room for exactly three items beside an empty shell.
        let mut builder = PartBuilder::new(3 * one + PART_OVERHEAD, 0);
        assert!(builder.is_empty());
        assert_eq!(builder.push(item.clone()), None);
        assert_eq!(builder.push(item.clone()), None);
        assert_eq!(builder.push(item.clone()), None);
        assert_eq!(builder.push(item.clone()).map(|full| full.len()), Some(3), "the fourth starts a part");
        assert_eq!(builder.finish().len(), 1);
        // A shell that takes the whole target still makes progress, one item a part.
        let mut builder = PartBuilder::new(64, 1_000);
        assert_eq!(builder.push(1u8), None);
        assert_eq!(builder.push(2u8), Some(vec![1u8]));
        assert_eq!(builder.finish(), [2u8]);
    }

    #[test]
    fn repeated_fields_and_flags_survive_paging() {
        let threads = SmsThreads {
            threads: (0..40)
                .map(|i| SmsThread {
                    id: format!("t{i}"),
                    addresses: vec!["x".repeat(2000)],
                    names: Vec::new(),
                    snippet: String::new(),
                    date: i,
                    unread: 0,
                    photo: None,
                })
                .collect(),
            app: Some("com.google.android.apps.messaging".into()),
        };
        let (_, parts) = round_trip(threads.clone(), 16 * 1024);
        assert!(parts > 1);
        assert!(threads.clone().split(16 * 1024).iter().all(|p| p.app == threads.app));

        let entries = StorageEntries {
            entries: (0..64)
                .map(|i| StorageEntry {
                    name: format!("{i}{}", "n".repeat(200)),
                    size: 1,
                    modified: 0,
                    is_dir: false,
                })
                .collect(),
            more: true,
        };
        let (_, parts) = round_trip(entries, 2048);
        assert!(parts > 1);

        let files = AgentFiles {
            files: (0..64)
                .map(|i| AgentFileHit {
                    path: format!("src/{i}/{}.rs", "f".repeat(100)),
                    ..Default::default()
                })
                .collect(),
            more: true,
        };
        assert!(round_trip(files, 1024).1 > 1);

        for thumbs in [0, 1, 7] {
            let list = PhotoThumbsList {
                thumbs: (0..thumbs)
                    .map(|i| PhotoThumb { id: format!("{i}"), data: vec![7; 90_000] })
                    .collect(),
            };
            round_trip(list, RPC_PART_TARGET);
        }
    }

    #[test]
    fn an_agent_page_keeps_its_thread_and_sends_its_asks_once() {
        let thread = AgentThread { id: "abc".into(), ..Default::default() };
        let page = AgentItems {
            thread: thread.clone(),
            items: (0..30)
                .map(|seq| AgentItem {
                    id: format!("i{seq}"),
                    seq,
                    text: Some("t".repeat(4000)),
                    ..Default::default()
                })
                .collect(),
            more_before: true,
            asks: vec![AgentAsk { token: "k1".into(), ..Default::default() }],
            questions: vec![AgentQuestion { token: "q1".into(), ..Default::default() }],
        };
        let parts = page.clone().split(32 * 1024);
        assert!(parts.len() > 2);
        assert!(parts.iter().all(|p| p.thread == thread && p.more_before));
        assert_eq!(parts[0].asks.len(), 1);
        assert_eq!(parts[0].questions.len(), 1);
        assert!(parts[1..].iter().all(|p| p.asks.is_empty() && p.questions.is_empty()));
        round_trip(page, 32 * 1024);

        let session = AgentSessionItems {
            session: session("s1"),
            items: (0..30)
                .map(|seq| AgentItem {
                    id: format!("i{seq}"),
                    seq,
                    text: Some("t".repeat(4000)),
                    ..Default::default()
                })
                .collect(),
            more_before: false,
        };
        assert!(round_trip(session, 32 * 1024).1 > 2);
    }

    #[test]
    fn every_paged_rpc_has_a_paged_body() {
        fn split_once<B: Paged>(body: B) -> usize {
            body.split(RPC_PART_TARGET).len()
        }
        // One body per paged op in `RPCS`, in its order.
        let counts = [
            split_once(CallLog { entries: Vec::new() }),
            split_once(ContactsList { contacts: Vec::new() }),
            split_once(SmsThreads { threads: Vec::new(), app: None }),
            split_once(SmsMessages { messages: Vec::new() }),
            split_once(PhotoAlbumsList::default()),
            split_once(PhotoItems::default()),
            split_once(PhotoThumbsList::default()),
            split_once(StorageEntries::default()),
            split_once(PhoneApps::default()),
            split_once(AgentThreads::default()),
            split_once(AgentItems::default()),
            split_once(AgentSessions::default()),
            split_once(AgentSessionItems { session: session("s1"), items: Vec::new(), more_before: false }),
            split_once(AgentFiles::default()),
            split_once(AgentCommands::default()),
            split_once(AgentTurns::default()),
            split_once(AgentTurnFiles::default()),
            split_once(crate::messages::AgentPatch::default()),
            split_once(AgentHits::default()),
            split_once(AgentThreads::default()),
            split_once(AgentFoundList::default()),
            split_once(AgentFoundItems::default()),
            split_once(crate::messages::AgentGit::default()),
            split_once(crate::messages::AgentGitPatch::default()),
            split_once(AgentGitReview::default()),
            split_once(AgentBranches::default()),
            split_once(AgentPrRows::default()),
        ];
        assert_eq!(counts.len(), RPCS.iter().filter(|op| op.paged).count());
        assert!(counts.iter().all(|&n| n == 1));
    }
}
