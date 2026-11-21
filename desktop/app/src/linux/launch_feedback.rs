// SPDX-License-Identifier: GPL-3.0-or-later
//! Ends the desktop's launch feedback for a notification click that opens no
//! window.
//!
//! GNOME Shell starts a startup sequence for every click on one of our
//! notifications (the `ActivationToken` it sends just before
//! `ActionInvoked`), and shows a busy pointer until a window is activated
//! with that token, or for 15 seconds. A button such as Mark as read opens no
//! window, and activating one with the token would raise it. GNOME's own
//! Wayland protocol has the request for this: `gtk_shell1.set_startup_id`,
//! what GTK sends for `gdk_notify_startup_complete()`, completes the sequence
//! and touches no window. Where the compositor has no `gtk_shell1` (KDE
//! Plasma, wlroots), nothing is done.

use std::ffi::CString;
use std::os::fd::OwnedFd;
use std::sync::{Arc, OnceLock, mpsc};

use wayland_client::backend::protocol::{AllowNull, Argument, ArgumentType, Interface, Message, MessageDesc};
use wayland_client::backend::smallvec::smallvec;
use wayland_client::backend::{Backend, ObjectData, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::__interfaces::WL_SURFACE_INTERFACE;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

/// `gtk_shell1` as far as it's used here (gtk-shell.xml in GTK and mutter):
/// the requests up to `set_startup_id`, and its one event.
static GTK_SHELL1: Interface = Interface {
    name: "gtk_shell1",
    version: 1,
    requests: &[
        MessageDesc {
            name: "get_gtk_surface",
            signature: &[ArgumentType::NewId, ArgumentType::Object(AllowNull::No)],
            since: 1,
            is_destructor: false,
            child_interface: None,
            arg_interfaces: &[&WL_SURFACE_INTERFACE],
        },
        MessageDesc {
            name: "set_startup_id",
            signature: &[ArgumentType::Str(AllowNull::Yes)],
            since: 1,
            is_destructor: false,
            child_interface: None,
            arg_interfaces: &[],
        },
    ],
    events: &[MessageDesc {
        name: "capabilities",
        signature: &[ArgumentType::Uint],
        since: 1,
        is_destructor: false,
        child_interface: None,
        arg_interfaces: &[],
    }],
    c_ptr: None,
};
const BIND: u16 = 0;
const SET_STARTUP_ID: u16 = 1;

static WORKER: OnceLock<mpsc::Sender<String>> = OnceLock::new();

/// Tells the desktop that the click `token` was given for is done, on a
/// thread of its own: a hung compositor holds up nothing else.
pub fn done(token: String) {
    if cfg!(test) || std::env::var_os("WAYLAND_DISPLAY").is_none_or(|d| d.is_empty()) {
        return;
    }
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        let spawned = std::thread::Builder::new().name("launch-feedback".into()).spawn(move || {
            for token in rx {
                if let Err(error) = Connection::connect_to_env()
                    .map_err(|e| e.to_string())
                    .and_then(|conn| complete(&conn, &token))
                {
                    tracing::debug!(error, "can't end the desktop's launch feedback");
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "can't start the launch feedback thread");
        }
        tx
    });
    let _ = worker.send(token);
}

/// Completes the startup sequence `token` names through `conn`. False when
/// the compositor has no `gtk_shell1`.
fn complete(conn: &Connection, token: &str) -> Result<bool, String> {
    let token = CString::new(token).map_err(|e| e.to_string())?;
    let (globals, _queue) = registry_queue_init::<Registry>(conn).map_err(|e| e.to_string())?;
    let Some(name) = globals.contents().with_list(|list| {
        list.iter().find(|g| g.interface == GTK_SHELL1.name && g.version >= 1).map(|g| g.name)
    }) else {
        return Ok(false);
    };
    let backend = conn.backend();
    let interface = CString::new(GTK_SHELL1.name).map_err(|e| e.to_string())?;
    let bind = Message {
        sender_id: globals.registry().id(),
        opcode: BIND,
        args: smallvec![
            Argument::Uint(name),
            Argument::Str(Some(Box::new(interface))),
            Argument::Uint(1),
            Argument::NewId(ObjectId::null()),
        ],
    };
    let shell = backend
        .send_request(bind, Some(Arc::new(IgnoreEvents)), Some((&GTK_SHELL1, 1)))
        .map_err(|e| e.to_string())?;
    let set = Message {
        sender_id: shell,
        opcode: SET_STARTUP_ID,
        args: smallvec![Argument::Str(Some(Box::new(token)))],
    };
    backend.send_request(set, None, None).map_err(|e| e.to_string())?;
    // Closing at once could drop the request unread: wait for the answer.
    conn.roundtrip().map_err(|e| e.to_string())?;
    Ok(true)
}

struct Registry;

impl Dispatch<WlRegistry, GlobalListContents> for Registry {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// `gtk_shell1` says what it can do when bound; nothing here needs it.
struct IgnoreEvents;

impl ObjectData for IgnoreEvents {
    fn event(self: Arc<Self>, _: &Backend, _: Message<ObjectId, OwnedFd>) -> Option<Arc<dyn ObjectData>> {
        None
    }

    fn destroyed(&self, _: ObjectId) {}
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::thread::JoinHandle;

    use super::*;

    enum Arg<'a> {
        Uint(u32),
        Str(&'a str),
    }

    fn send(stream: &mut UnixStream, object: u32, opcode: u16, args: &[Arg<'_>]) {
        let mut body = Vec::new();
        for arg in args {
            match arg {
                Arg::Uint(v) => body.extend_from_slice(&v.to_ne_bytes()),
                Arg::Str(s) => {
                    body.extend_from_slice(&(s.len() as u32 + 1).to_ne_bytes());
                    body.extend_from_slice(s.as_bytes());
                    body.push(0);
                    body.resize(body.len().next_multiple_of(4), 0);
                }
            }
        }
        let size = 8 + body.len() as u32;
        stream.write_all(&object.to_ne_bytes()).unwrap();
        stream.write_all(&((size << 16) | u32::from(opcode)).to_ne_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    }

    struct Args<'a>(&'a [u8]);

    impl Args<'_> {
        fn uint(&mut self) -> u32 {
            let (head, rest) = self.0.split_at(4);
            self.0 = rest;
            u32::from_ne_bytes(head.try_into().unwrap())
        }

        fn string(&mut self) -> String {
            let len = self.uint() as usize;
            let (head, rest) = self.0.split_at(len.next_multiple_of(4));
            self.0 = rest;
            String::from_utf8(head[..len - 1].to_vec()).unwrap()
        }
    }

    /// A compositor offering `globals`: it answers `sync`, and says what was
    /// bound and asked of `gtk_shell1` once the client has gone.
    fn compositor(mut stream: UnixStream, globals: &'static [&'static str]) -> JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let (mut registry, mut shell, mut seen) = (None, None, Vec::new());
            let mut header = [0u8; 8];
            while stream.read_exact(&mut header).is_ok() {
                let object = u32::from_ne_bytes(header[..4].try_into().unwrap());
                let word = u32::from_ne_bytes(header[4..].try_into().unwrap());
                let mut body = vec![0u8; (word >> 16) as usize - 8];
                stream.read_exact(&mut body).unwrap();
                let mut args = Args(&body);
                match (object, word & 0xffff) {
                    // wl_display.sync: wl_callback.done, then wl_display.delete_id.
                    (1, 0) => {
                        let callback = args.uint();
                        send(&mut stream, callback, 0, &[Arg::Uint(0)]);
                        send(&mut stream, 1, 1, &[Arg::Uint(callback)]);
                    }
                    // wl_display.get_registry: the globals.
                    (1, 1) => {
                        let id = args.uint();
                        registry = Some(id);
                        for (name, interface) in (1..).zip(globals.iter().copied()) {
                            send(&mut stream, id, 0, &[Arg::Uint(name), Arg::Str(interface), Arg::Uint(7)]);
                        }
                    }
                    (o, 0) if Some(o) == registry => {
                        let (_, interface, version, id) =
                            (args.uint(), args.string(), args.uint(), args.uint());
                        seen.push(format!("bind {interface} v{version}"));
                        shell = Some(id);
                        // gtk_shell1.capabilities, as mutter sends it on bind.
                        send(&mut stream, id, 0, &[Arg::Uint(0)]);
                    }
                    (o, 1) if Some(o) == shell => seen.push(format!("set_startup_id {}", args.string())),
                    (o, op) => seen.push(format!("unexpected {o}.{op}")),
                }
            }
            seen
        })
    }

    const TOKEN: &str = "0b5f6c3e-1d2a-4c8e-9f70-6a2b3c4d5e6f";

    #[test]
    fn the_startup_sequence_is_completed_without_a_window() {
        let (client, server) = UnixStream::pair().unwrap();
        let compositor = compositor(server, &["wl_compositor", "gtk_shell1", "wl_seat"]);
        let conn = Connection::from_socket(client).unwrap();
        assert_eq!(complete(&conn, TOKEN), Ok(true));
        drop(conn);
        assert_eq!(
            compositor.join().unwrap(),
            vec!["bind gtk_shell1 v1".to_owned(), format!("set_startup_id {TOKEN}")],
            "only gtk_shell1, and no surface"
        );
    }

    #[test]
    fn without_gtk_shell_nothing_is_asked() {
        let (client, server) = UnixStream::pair().unwrap();
        let compositor = compositor(server, &["wl_compositor", "xdg_activation_v1"]);
        let conn = Connection::from_socket(client).unwrap();
        assert_eq!(complete(&conn, TOKEN), Ok(false));
        drop(conn);
        assert_eq!(compositor.join().unwrap(), Vec::<String>::new());
    }
}
