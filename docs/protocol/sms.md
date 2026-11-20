# Messages service (protocol v1)

> Status: **v1.** Builds on [protocol v1](v1.md). License: CC BY 4.0.

A PC reads a phone's text messages (SMS and MMS) and sends texts through
it. The phone keeps the messages; a PC asks for what it shows and keeps it
only in memory. New-message alerts come from the phone's own notifications
([notifications](notifications.md)), not from this service.

## 1. Capabilities

| ID | Offered by | Meaning |
|---|---|---|
| `sms.read` | phone | Lists conversations and messages (needs the SMS permission) |
| `sms.send` | phone | Sends texts when a PC asks |
| `sms.show` | PC | Shows messages, and wants `sms.changed` |

Both devices must allow it: the `messages` device toggle, on by default,
on each side.

## 2. Messages

The requests are RPCs (PC → phone, [protocol v1](v1.md) §4.3), each answered
`ok` with the answer below, or `error`; `sms.changed` goes one way on the
`sms` lane (phone → PC).

```
RPC sms/threads    t = "sms.threads"    b = { limit: uint }                          // 1 to 100
  answer (paged): { threads: [ thread ], ? app: text }   // newest first; app: the default SMS app's package, in every part

RPC sms/messages   t = "sms.messages"   b = { thread: text, ? before: int, limit: uint }
  answer (paged): { messages: [ message ] }              // newest first

RPC sms/send       t = "sms.send"       b = { to: [ text ], ? body: text, ? attachments: [ attachment ] }
  answer: ok

RPC sms/part       t = "sms.part"       b = { id: text }
  answer: { mime: text, data: bytes }                    // one frame

t = "sms.changed"   b = { ? thread: text }                                           // phone → PC

attachment = {
  mime: "image/jpeg" | "image/png" | "image/gif" | "image/webp"
      | "audio/aac" | "audio/mp4" | "audio/mpeg" | "audio/ogg"
      | "audio/opus" | "audio/amr" | "audio/3gpp" | "audio/wav" | "audio/x-m4a",
  data: bytes,            // 1 B to 900 KiB
}

thread = {
  id: text,
  addresses: [ text ],    // The other people's numbers; more than one is a group
  ? names: [ text ],      // Their contact names, in the same order ("" when not a contact)
  ? snippet: text,        // The latest message's text, or a description like "Picture"
  date: int,              // The latest message, in Unix milliseconds
  ? unread: uint,
  ? photo: bytes,         // One-person conversations: the contact's photo, a JPEG of at most 16 KiB
}

message = {
  id: text,
  thread: text,
  ? address: text,        // Who sent it (received), or the recipient (sent)
  ? body: text,
  date: int,              // Unix milliseconds
  ? outgoing: bool,       // Sent from the phone
  ? status: "sent" | "pending" | "failed",
  ? parts: [ { id: text, mime: text, ? size: uint } ],   // Pictures and other attachments
}
```

### 2.1 Reading

`sms.messages` pages back through a conversation: `before` is the oldest
`date` already shown. A page larger than a frame comes in parts
([protocol v1](v1.md) §4.4).

`sms.part` fetches an attachment; parts larger than 900 KiB are refused
with `BUSY`.

### 2.2 Sending

`sms.send` sends `body` (at most 8 KiB) and/or `attachments` (at most 4 media
attachments, image or voice/audio, each at most 900 KiB) to `to` (1 to 20
numbers). When `attachments` is empty, `body` must not be blank and is sent as
SMS; when one or more attachments are included, the phone sends the message as
MMS (`sendMultimediaMessage`).
Errors: `DENIED` when messages are off for the PC, `UNSUPPORTED` when the phone
doesn't offer `sms.send`, `INTERNAL` when it couldn't send.

### 2.3 Changes

The phone sends `sms.changed` to PCs that offer `sms.show` when its
messages change (a text arrived or was sent), with the conversation when
it knows it. PCs then ask again for what they show.

## 3. Limits of this version

- Texts are sent to one number at a time; replying in a group needs MMS
  and isn't part of this version.
- Phones can't mark messages as read for other apps: Android lets only the
  default SMS app change the messages store.
- Android 15 and later keep texts with one-time codes from other apps for
  a few hours; such conversations look empty in the meantime.

## 4. Privacy

Numbers, names, text and pictures are never logged ([protocol v1](v1.md) §11).
