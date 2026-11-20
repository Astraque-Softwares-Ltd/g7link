// SPDX-License-Identifier: GPL-3.0-or-later
//! `Agents` and `AgentTimeline`: the Agents page. `Agents` holds the lists,
//! the open thread or session and the setup as properties (JSON QML reads)
//! and takes the page's requests; `AgentTimeline` is the open view's rows as
//! a list model. The rows are computed off the GUI thread
//! (`agents::rerender`); the model applies the exact changes by key, so
//! streaming text changes only its last block.

use std::{pin::Pin, sync::Arc};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QUrl, QVariant};
use serde_json::json;

use crate::{
    agents::{self, ModelRow, Role, RoleValue, RowOp, UiUpdate},
    state::Changes,
};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        /// "idle", "connecting", "connected", "not_running", "not_installed" or "failed".
        #[qproperty(QString, status)]
        /// `{ status, following, agents: [...], folders: [...] }` (JSON).
        #[qproperty(QString, setup)]
        /// Threads, waiting first, then the newest (JSON).
        #[qproperty(QString, threads)]
        /// Sessions started on this PC outside G7, in the host's order (JSON).
        #[qproperty(QString, sessions)]
        /// Threads waiting for an answer and sessions that certainly need you.
        #[qproperty(i32, needs_you)]
        /// "", "thread" or "session".
        #[qproperty(QString, open_kind)]
        #[qproperty(QString, open_id)]
        /// The open row, with the same fields as its list row (JSON).
        #[qproperty(QString, open_info)]
        /// The open thread's oldest waiting request (JSON object, `{}` for none).
        #[qproperty(QString, ask)]
        /// How many more of its requests wait behind that one.
        #[qproperty(i32, asks_waiting)]
        /// The open view's newest tool steps, oldest first (JSON).
        #[qproperty(QString, steps)]
        /// The files the open view's edits changed (JSON).
        #[qproperty(QString, changes)]
        /// Earlier items can be read.
        #[qproperty(bool, more)]
        /// A read is running.
        #[qproperty(bool, loading)]
        /// A request is running.
        #[qproperty(bool, busy)]
        /// Where the save dialog suggests saving the waiting export, as a
        /// file URL ("" while none waits).
        #[qproperty(QString, export_file)]
        /// The agent host is out of date and threads run on it; restarting
        /// it finishes the update.
        #[qproperty(bool, outdated)]
        /// The host can't list sessions started on this PC until it's updated.
        #[qproperty(bool, sessions_unsupported)]
        /// The open thread's request can be denied with words for the agent.
        #[qproperty(bool, can_say_why)]
        /// The user hid the thread list (it stays hidden across restarts).
        #[qproperty(bool, list_hidden)]
        /// The user hid the details beside the transcript.
        #[qproperty(bool, inspector_hidden)]
        /// The question the dock shows rather than a request (JSON, `{}` for none).
        #[qproperty(QString, question)]
        /// The open thread's composer around its text: the model control,
        /// the queue, the images, the limit line, a draft offered (JSON).
        #[qproperty(QString, composer)]
        /// The `@` or `/` list over the composer (JSON, `{}` for none).
        #[qproperty(QString, popup)]
        /// Text the composer must take: `{ gen, n, text, cursor }` (JSON).
        #[qproperty(QString, composer_set)]
        /// The new-thread form's model control (JSON).
        #[qproperty(QString, new_picker)]
        type Agents = super::AgentsRust;

        /// The page is on screen (or left).
        #[qinvokable]
        fn page_visible(self: &Agents, visible: bool);
        /// The window is the active one (or stopped being it, or was hidden).
        #[qinvokable]
        fn window_active(self: &Agents, active: bool);
        #[qinvokable]
        fn start_host(self: &Agents);
        /// Restarts an outdated host now (the user agreed its threads stop).
        #[qinvokable]
        fn restart_host(self: &Agents);
        #[qinvokable]
        fn refresh(self: &Agents);
        /// Adds a folder by its local path (after the user confirmed it).
        #[qinvokable]
        fn add_folder(self: &Agents, path: &QString);
        #[qinvokable]
        fn remove_folder(self: &Agents, id: &QString);
        /// Adds the folder a session runs in (confirmed), then continues the session.
        #[qinvokable]
        fn add_folder_and_continue(self: &Agents, path: &QString, session_id: &QString);
        /// Turns following sessions started outside G7 on or off.
        #[qinvokable]
        fn set_following(self: &Agents, on: bool);
        #[qinvokable]
        fn open_thread(self: &Agents, id: &QString);
        #[qinvokable]
        fn open_session(self: &Agents, id: &QString);
        #[qinvokable]
        fn close(self: &Agents);
        /// Starts a thread (`mode`: "supervised" or "auto_edit") and opens it.
        #[qinvokable]
        fn start(self: &Agents, agent: &QString, folder_id: &QString, prompt: &QString, mode: &QString);
        /// Sends the composer's message; while the agent works it steers or
        /// waits its turn by the user's setting, `other` (Alt+Enter) the other.
        #[qinvokable]
        fn send(self: &Agents, text: &QString, other: bool);
        /// Stops the open thread's turn.
        #[qinvokable]
        fn stop(self: &Agents);
        /// Answers an approval: "allow", "deny" or "allow_thread".
        #[qinvokable]
        fn answer(self: &Agents, token: &QString, decision: &QString);
        #[qinvokable]
        fn delete_thread(self: &Agents, id: &QString);
        #[qinvokable]
        fn open_in_terminal(self: &Agents);
        #[qinvokable]
        fn load_earlier(self: &Agents);
        #[qinvokable]
        fn continue_session(self: &Agents, id: &QString);
        #[qinvokable]
        fn copy(self: &Agents, text: &QString);
        /// Copies the open thread or session as Markdown.
        #[qinvokable]
        fn copy_markdown(self: &Agents);
        /// Gets the open thread or session as Markdown; `exportFile` then
        /// says where the save dialog starts.
        #[qinvokable]
        fn export_markdown(self: &Agents);
        /// Saves the waiting export to the file the dialog chose.
        #[qinvokable]
        fn save_export(self: &Agents, url: &QString);
        /// The save dialog closed without saving.
        #[qinvokable]
        fn drop_export(self: &Agents);
        /// Opens or closes a row (a tool, a group, a thought, the fold, a file).
        #[qinvokable]
        fn toggle(self: &Agents, key: &QString);
        /// Wraps a code block's long lines, or stops.
        #[qinvokable]
        fn toggle_wrap(self: &Agents, key: &QString);
        /// Copies a row's source: a code block, a command, an output; with
        /// `i:<item id>`, a whole message. False when there's nothing.
        #[qinvokable]
        fn copy_text(self: &Agents, key: &QString) -> bool;
        /// The composer's text or cursor changed (`typed`: by the user, not
        /// the page). The cursor is in UTF-16 units, as QML counts.
        #[qinvokable]
        fn composer_edited(self: &Agents, text: &QString, cursor: i32, typed: bool);
        /// Moves the `@` or `/` list's selection.
        #[qinvokable]
        fn popup_move(self: &Agents, by: i32);
        /// Inserts a row of the list (`-1`: the selected one): `{ text,
        /// cursor, action }` for the composer ("" when nothing was chosen).
        #[qinvokable]
        fn popup_choose(self: &Agents, index: i32) -> QString;
        #[qinvokable]
        fn popup_close(self: &Agents);
        /// Up (`older`) or Down through prompts sent from this PC:
        /// `{ text }`, or "" when there's nothing that way.
        #[qinvokable]
        fn history_step(self: &Agents, older: bool) -> QString;
        /// Takes the draft another device offered.
        #[qinvokable]
        fn use_draft(self: &Agents);
        /// Adds pictures to the next message, as a JSON list of file URLs.
        #[qinvokable]
        fn add_images(self: &Agents, urls: &QString);
        /// Adds the clipboard's image to the next message; false when it has none.
        #[qinvokable]
        fn paste_image(self: &Agents) -> bool;
        #[qinvokable]
        fn remove_image(self: &Agents, id: &QString);
        /// The new-thread form shows this agent and folder.
        #[qinvokable]
        fn new_form(self: &Agents, agent: &QString, folder: &QString);
        /// A pick in the model control (`what`: "model", "custom", "effort",
        /// "mode"), for the new-thread form or the open thread.
        #[qinvokable]
        fn pick(self: &Agents, new_form: bool, what: &QString, value: &QString);
        /// While it works, a new message "steer"s it or "queue"s.
        #[qinvokable]
        fn set_follow_up(self: &Agents, word: &QString);
        /// "send_now", "remove" or "edit" (with `text`) a queued message.
        #[qinvokable]
        fn queue(self: &Agents, op: &QString, id: &QString, text: &QString);
        /// Implements a plan, asking before edits and commands
        /// ("supervised") or before commands ("auto_edit").
        #[qinvokable]
        fn implement(self: &Agents, item_id: &QString, mode: &QString);
        /// Picks an option of the shown ask (one past them: "Something else").
        #[qinvokable]
        fn question_pick(self: &Agents, index: i32);
        /// The secret ask's masked field changed.
        #[qinvokable]
        fn question_secret(self: &Agents, text: &QString);
        /// The next ask (`next`) or the one before.
        #[qinvokable]
        fn question_step(self: &Agents, next: bool);
        #[qinvokable]
        fn question_send(self: &Agents);
        #[qinvokable]
        fn question_dismiss(self: &Agents);
        /// A folder's "Use my MCP servers here".
        #[qinvokable]
        fn set_folder_mcp(self: &Agents, id: &QString, on: bool);
        /// "Keep this PC awake while an agent works".
        #[qinvokable]
        fn set_keep_awake(self: &Agents, on: bool);
        /// The composer gained or lost the keyboard.
        #[qinvokable]
        fn composer_focus(self: &Agents, focused: bool);
        /// Denies the request with the user's words for the agent.
        #[qinvokable]
        fn deny_with_message(self: &Agents, token: &QString, text: &QString);
        /// Where a row's link `index` goes, as JSON `{ href, text, shown }`
        /// ("" for none): `href` with its host in ASCII, `shown` when its
        /// text is its target (it opens without asking).
        #[qinvokable]
        fn link_target(self: &Agents, key: &QString, index: i32) -> QString;
        /// Opens a row's link `index` (after the page showed where it goes).
        #[qinvokable]
        fn open_link(self: &Agents, key: &QString, index: i32);
        /// The theme's colours for code, as JSON `{ keyword: "#…", … }`.
        #[qinvokable]
        fn set_code_colors(self: &Agents, json: &QString);
        /// Hides or shows the thread list; remembered.
        #[qinvokable]
        fn hide_list(self: &Agents, hidden: bool);
        /// Hides or shows the details beside the transcript; remembered.
        #[qinvokable]
        fn hide_inspector(self: &Agents, hidden: bool);
    }

    impl cxx_qt::Threading for Agents {}
    impl cxx_qt::Initialize for Agents {}

    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;

        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, count)]
        type AgentTimeline = super::AgentTimelineRust;
    }

    // Inherited from QAbstractListModel.
    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginInsertRows"]
        unsafe fn begin_insert_rows(
            self: Pin<&mut AgentTimeline>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        #[inherit]
        #[cxx_name = "endInsertRows"]
        unsafe fn end_insert_rows(self: Pin<&mut AgentTimeline>);
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        unsafe fn begin_remove_rows(
            self: Pin<&mut AgentTimeline>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        unsafe fn end_remove_rows(self: Pin<&mut AgentTimeline>);
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut AgentTimeline>);
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut AgentTimeline>);
        #[inherit]
        fn index(self: &AgentTimeline, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;

        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut AgentTimeline>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );
    }

    unsafe extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &AgentTimeline, index: &QModelIndex, role: i32) -> QVariant;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &AgentTimeline) -> QHash_i32_QByteArray;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &AgentTimeline, parent: &QModelIndex) -> i32;
        /// The row of a key, or -1.
        #[qinvokable]
        #[cxx_name = "rowOfKey"]
        fn row_of_key(self: &AgentTimeline, key: &QString) -> i32;
    }

    impl cxx_qt::Threading for AgentTimeline {}
    impl cxx_qt::Initialize for AgentTimeline {}
}

#[derive(Default)]
pub struct AgentsRust {
    status: QString,
    setup: QString,
    threads: QString,
    sessions: QString,
    needs_you: i32,
    open_kind: QString,
    open_id: QString,
    open_info: QString,
    ask: QString,
    asks_waiting: i32,
    steps: QString,
    changes: QString,
    more: bool,
    loading: bool,
    busy: bool,
    export_file: QString,
    outdated: bool,
    sessions_unsupported: bool,
    can_say_why: bool,
    list_hidden: bool,
    inspector_hidden: bool,
    question: QString,
    composer: QString,
    popup: QString,
    composer_set: QString,
    new_picker: QString,
    /// What was last shown, to set only what changed.
    last: Option<agents::View>,
}

impl cxx_qt::Initialize for qobject::Agents {
    fn initialize(self: Pin<&mut Self>) {
        super::subscribe(self.qt_thread(), Changes::AGENTS, Self::refresh_view);
        self.refresh_view();
    }
}

impl qobject::Agents {
    fn refresh_view(mut self: Pin<&mut Self>) {
        let view = agents::view();
        let last = self.rust().last.clone();
        if last.as_ref() == Some(&view) {
            return;
        }
        let differs = |f: fn(&agents::View) -> &String| last.as_ref().is_none_or(|l| f(l) != f(&view));
        if differs(|v| &v.status) {
            self.as_mut().set_status(QString::from(&view.status));
        }
        if differs(|v| &v.setup) {
            self.as_mut().set_setup(QString::from(&view.setup));
        }
        if differs(|v| &v.threads) {
            self.as_mut().set_threads(QString::from(&view.threads));
        }
        if differs(|v| &v.sessions) {
            self.as_mut().set_sessions(QString::from(&view.sessions));
        }
        if differs(|v| &v.open_kind) {
            self.as_mut().set_open_kind(QString::from(&view.open_kind));
        }
        if differs(|v| &v.open_id) {
            self.as_mut().set_open_id(QString::from(&view.open_id));
        }
        if differs(|v| &v.open_info) {
            self.as_mut().set_open_info(QString::from(&view.open_info));
        }
        if differs(|v| &v.ask) {
            self.as_mut().set_ask(QString::from(&view.ask));
        }
        if differs(|v| &v.steps) {
            self.as_mut().set_steps(QString::from(&view.steps));
        }
        if differs(|v| &v.changes) {
            self.as_mut().set_changes(QString::from(&view.changes));
        }
        if differs(|v| &v.question) {
            self.as_mut().set_question(QString::from(&view.question));
        }
        if differs(|v| &v.composer) {
            self.as_mut().set_composer(QString::from(&view.composer));
        }
        if differs(|v| &v.popup) {
            self.as_mut().set_popup(QString::from(&view.popup));
        }
        if differs(|v| &v.composer_set) {
            self.as_mut().set_composer_set(QString::from(&view.composer_set));
        }
        if differs(|v| &v.new_picker) {
            self.as_mut().set_new_picker(QString::from(&view.new_picker));
        }
        if differs(|v| &v.export_path) {
            let url = if view.export_path.is_empty() {
                QString::default()
            } else {
                QUrl::from_local_file(&QString::from(&view.export_path)).to_qstring()
            };
            self.as_mut().set_export_file(url);
        }
        self.as_mut().set_needs_you(view.needs_you);
        self.as_mut().set_asks_waiting(view.asks_waiting);
        self.as_mut().set_outdated(view.outdated);
        self.as_mut().set_sessions_unsupported(view.sessions_unsupported);
        self.as_mut().set_can_say_why(view.can_say_why);
        self.as_mut().set_list_hidden(view.list_hidden);
        self.as_mut().set_inspector_hidden(view.inspector_hidden);
        self.as_mut().set_more(view.more);
        self.as_mut().set_loading(view.loading);
        self.as_mut().set_busy(view.busy);
        self.as_mut().rust_mut().last = Some(view);
    }

    pub fn page_visible(&self, visible: bool) {
        agents::page_visible(visible);
    }

    pub fn window_active(&self, active: bool) {
        agents::window_active(active);
    }

    pub fn start_host(&self) {
        agents::start_host();
    }

    pub fn restart_host(&self) {
        agents::restart_host();
    }

    pub fn refresh(&self) {
        agents::refresh();
    }

    pub fn add_folder(&self, path: &QString) {
        let path = String::from(path);
        if !path.is_empty() {
            agents::add_folder(path);
        }
    }

    pub fn remove_folder(&self, id: &QString) {
        agents::remove_folder(String::from(id));
    }

    pub fn add_folder_and_continue(&self, path: &QString, session_id: &QString) {
        let path = String::from(path);
        if !path.is_empty() {
            agents::add_folder_and_continue(path, String::from(session_id));
        }
    }

    pub fn set_following(&self, on: bool) {
        agents::set_following(on);
    }

    pub fn open_thread(&self, id: &QString) {
        agents::open_thread(String::from(id));
    }

    pub fn open_session(&self, id: &QString) {
        agents::open_session(String::from(id));
    }

    pub fn close(&self) {
        agents::close();
    }

    pub fn start(&self, agent: &QString, folder_id: &QString, prompt: &QString, mode: &QString) {
        let prompt = String::from(prompt);
        if prompt.trim().is_empty() {
            return;
        }
        agents::start_thread(String::from(agent), String::from(folder_id), prompt, String::from(mode));
    }

    pub fn send(&self, text: &QString, other: bool) {
        agents::send(String::from(text), other);
    }

    pub fn stop(&self) {
        agents::stop_thread();
    }

    pub fn answer(&self, token: &QString, decision: &QString) {
        agents::answer(String::from(token), &String::from(decision));
    }

    pub fn delete_thread(&self, id: &QString) {
        agents::delete_thread(String::from(id));
    }

    pub fn open_in_terminal(&self) {
        agents::open_in_terminal();
    }

    pub fn load_earlier(&self) {
        agents::load_earlier();
    }

    pub fn continue_session(&self, id: &QString) {
        agents::continue_session(String::from(id));
    }

    /// The button that asked says "Copied" itself.
    pub fn copy(&self, text: &QString) {
        agents::copy(String::from(text));
    }

    pub fn copy_markdown(&self) {
        agents::copy_markdown();
    }

    pub fn export_markdown(&self) {
        agents::export_markdown();
    }

    pub fn save_export(&self, url: &QString) {
        match QUrl::from(url).to_local_file().map(|p| String::from(&p)).filter(|p| !p.is_empty()) {
            Some(path) => agents::save_export(path.into()),
            None => agents::drop_export(),
        }
    }

    pub fn drop_export(&self) {
        agents::drop_export();
    }

    pub fn toggle(&self, key: &QString) {
        agents::toggle(String::from(key));
    }

    pub fn toggle_wrap(&self, key: &QString) {
        agents::toggle_wrap(String::from(key));
    }

    pub fn copy_text(&self, key: &QString) -> bool {
        match agents::copy_text(&String::from(key)) {
            Some(text) => {
                agents::copy(text);
                true
            }
            None => false,
        }
    }

    pub fn composer_edited(&self, text: &QString, cursor: i32, typed: bool) {
        agents::composer_edited(String::from(text), cursor.max(0).unsigned_abs(), typed);
    }

    pub fn popup_move(&self, by: i32) {
        agents::popup_move(by.clamp(-1, 1));
    }

    pub fn popup_choose(&self, index: i32) -> QString {
        match agents::popup_choose(index) {
            Some(c) => {
                QString::from(&json!({ "text": c.text, "cursor": c.cursor, "action": c.action }).to_string())
            }
            None => QString::default(),
        }
    }

    pub fn popup_close(&self) {
        agents::popup_close();
    }

    pub fn history_step(&self, older: bool) -> QString {
        match agents::history_step(older) {
            Some(text) => QString::from(&json!({ "text": text }).to_string()),
            None => QString::default(),
        }
    }

    pub fn use_draft(&self) {
        agents::use_draft();
    }

    /// Only local files are passed on; the host reads them as images or refuses them.
    pub fn add_images(&self, urls: &QString) {
        let urls: Vec<String> = serde_json::from_str(&String::from(urls)).unwrap_or_default();
        let paths: Vec<String> = urls
            .iter()
            .take(g7_agents::api::MAX_IMAGES * 2)
            .filter_map(|u| QUrl::from(&QString::from(u.as_str())).to_local_file().map(|p| String::from(&p)))
            .filter(|p| !p.is_empty())
            .collect();
        if !paths.is_empty() {
            agents::add_images(paths);
        }
    }

    pub fn paste_image(&self) -> bool {
        agents::paste_image()
    }

    pub fn remove_image(&self, id: &QString) {
        agents::remove_image(String::from(id));
    }

    pub fn new_form(&self, agent: &QString, folder: &QString) {
        agents::new_form(String::from(agent), String::from(folder));
    }

    pub fn pick(&self, new_form: bool, what: &QString, value: &QString) {
        agents::pick(new_form, &String::from(what), String::from(value));
    }

    pub fn set_follow_up(&self, word: &QString) {
        agents::set_follow_up(&String::from(word));
    }

    pub fn queue(&self, op: &QString, id: &QString, text: &QString) {
        agents::queue(&String::from(op), String::from(id), String::from(text));
    }

    pub fn implement(&self, item_id: &QString, mode: &QString) {
        agents::implement(String::from(item_id), &String::from(mode));
    }

    pub fn question_pick(&self, index: i32) {
        if let Ok(index) = usize::try_from(index) {
            agents::question_pick(index);
        }
    }

    pub fn question_secret(&self, text: &QString) {
        agents::question_secret(String::from(text));
    }

    pub fn question_step(&self, next: bool) {
        agents::question_step(next);
    }

    pub fn question_send(&self) {
        agents::question_send();
    }

    pub fn question_dismiss(&self) {
        agents::question_dismiss();
    }

    pub fn set_folder_mcp(&self, id: &QString, on: bool) {
        agents::set_folder_mcp(String::from(id), on);
    }

    pub fn set_keep_awake(&self, on: bool) {
        agents::set_keep_awake(on);
    }

    pub fn composer_focus(&self, focused: bool) {
        agents::composer_focus(focused);
    }

    pub fn deny_with_message(&self, token: &QString, text: &QString) {
        agents::deny_with_message(String::from(token), String::from(text));
    }

    pub fn link_target(&self, key: &QString, index: i32) -> QString {
        let Some(link) = usize::try_from(index).ok().and_then(|i| agents::link(&String::from(key), i)) else {
            return QString::default();
        };
        let json = json!({ "href": ascii_url(&link.href), "text": link.text, "shown": link.shown });
        QString::from(&json.to_string())
    }

    pub fn open_link(&self, key: &QString, index: i32) {
        if let Some(link) = usize::try_from(index).ok().and_then(|i| agents::link(&String::from(key), i)) {
            agents::open_link(ascii_url(&link.href));
        }
    }

    pub fn set_code_colors(&self, json: &QString) {
        agents::set_code_colors(&String::from(json));
    }

    pub fn hide_list(&self, hidden: bool) {
        agents::set_panels(Some(hidden), None);
    }

    pub fn hide_inspector(&self, hidden: bool) {
        agents::set_panels(None, Some(hidden));
    }
}

/// `href` as the page shows and opens it: an internationalised host in its
/// ASCII (punycode) form, everything else percent-encoded.
fn ascii_url(href: &str) -> String {
    let url = QUrl::from(&QString::from(href));
    if !url.is_valid() {
        return href.to_owned();
    }
    String::from_utf8_lossy(url.to_encoded().as_slice()).into_owned()
}

// ---- The timeline ----

#[derive(Default)]
pub struct AgentTimelineRust {
    count: i32,
    rows: Vec<Arc<ModelRow>>,
}

const USER_ROLE: i32 = 0x0100;

fn role_id(role: Role) -> i32 {
    USER_ROLE + role.index() as i32
}

fn role_value(row: &ModelRow, role: Role) -> QVariant {
    match row.get(role) {
        RoleValue::Text(t) => QVariant::from(&QString::from(t.as_str())),
        RoleValue::Int(n) => QVariant::from(n),
        RoleValue::Real(n) => QVariant::from(n),
        RoleValue::Flag(b) => QVariant::from(b),
    }
}

impl cxx_qt::Initialize for qobject::AgentTimeline {
    fn initialize(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        agents::attach_ui(move || qt.queue(|object| object.apply_updates()).is_ok());
    }
}

impl qobject::AgentTimeline {
    fn apply_updates(mut self: Pin<&mut Self>) {
        for update in agents::take_ui() {
            match update {
                UiUpdate::Ops(ops) => {
                    for op in ops {
                        self.as_mut().apply_op(op);
                    }
                }
                UiUpdate::Reset(rows) => self.as_mut().reset(rows),
            }
        }
        let count = i32::try_from(self.rows.len()).unwrap_or(i32::MAX);
        self.set_count(count);
    }

    fn reset(mut self: Pin<&mut Self>, rows: Vec<Arc<ModelRow>>) {
        // SAFETY: begin/end are balanced and the rows change between them.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().rows = rows;
            self.as_mut().end_reset_model();
        }
    }

    /// One change, with paired begin/end notifications and the exact roles.
    /// The changes were made against these rows; one out of range (which
    /// can't happen) is skipped rather than break the model.
    fn apply_op(mut self: Pin<&mut Self>, op: RowOp) {
        let root = QModelIndex::default();
        let len = self.rows.len();
        match op {
            RowOp::Remove { at, count } => {
                if count == 0 || at + count > len {
                    return;
                }
                // SAFETY: rows at..at+count exist; begin/end are balanced.
                unsafe {
                    self.as_mut().begin_remove_rows(&root, at as i32, (at + count - 1) as i32);
                    self.as_mut().rust_mut().rows.drain(at..at + count);
                    self.as_mut().end_remove_rows();
                }
            }
            RowOp::Insert { at, rows } => {
                if rows.is_empty() || at > len {
                    return;
                }
                let last = (at + rows.len() - 1) as i32;
                // SAFETY: at <= len; begin/end are balanced.
                unsafe {
                    self.as_mut().begin_insert_rows(&root, at as i32, last);
                    self.as_mut().rust_mut().rows.splice(at..at, rows);
                    self.as_mut().end_insert_rows();
                }
            }
            RowOp::Change { at, row, roles } => {
                if at >= len {
                    return;
                }
                self.as_mut().rust_mut().rows[at] = row;
                let index = self.index(at as i32, 0, &root);
                let mut ids = QList::<i32>::default();
                for role in roles {
                    ids.append(role_id(role));
                }
                self.as_mut().data_changed(&index, &index, &ids);
            }
        }
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let row = usize::try_from(index.row()).ok().and_then(|r| self.rows.get(r));
        let role = usize::try_from(role - USER_ROLE).ok().and_then(|r| Role::ALL.get(r));
        match (row, role) {
            (Some(row), Some(role)) => role_value(row, *role),
            _ => QVariant::default(),
        }
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for role in Role::ALL {
            roles.insert(role_id(role), QByteArray::from(role.name()));
        }
        roles
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        i32::try_from(self.rows.len()).unwrap_or(i32::MAX)
    }

    pub fn row_of_key(&self, key: &QString) -> i32 {
        let key = String::from(key);
        self.rows.iter().position(|r| r.key() == key).map_or(-1, |i| i as i32)
    }
}

#[cfg(test)]
mod tests {
    use g7_agents_render::{Row, RowKind, RowStatus, ToolClass, ToolLabel};

    use super::*;

    #[test]
    fn roles_cover_the_row() {
        let row = Row {
            key: "i:toolu_1".into(),
            kind: RowKind::Tool {
                label: ToolLabel { class: ToolClass::Command, object: "cargo test".into(), more: 0 },
                status: RowStatus::Running,
                has_detail: true,
                has_output: false,
                open: false,
                adds: 0,
                dels: 0,
                ms: 0,
            },
            turn: 3,
            depth: 1,
            item_id: "toolu_1".into(),
            seq: (1 << 40) | 3,
        };
        let row =
            agents::model_row(&row, agents::Extra::default(), "cargo test", &agents::CodePalette::default());
        let get = |role| role_value(&row, role);
        assert_eq!(get(Role::Key).value::<QString>().map(String::from).as_deref(), Some("i:toolu_1"));
        assert_eq!(get(Role::Status).value::<QString>().map(String::from).as_deref(), Some("running"));
        assert_eq!(get(Role::HasDetail).value::<bool>(), Some(true));
        assert_eq!(get(Role::Turn).value::<i32>(), Some(3));
        assert_eq!(get(Role::Ms).value::<f64>(), Some(0.0));
        assert_eq!(
            get(Role::Styled).value::<QString>().map(String::from).as_deref(),
            Some("cargo&nbsp;test")
        );
        assert_eq!(role_id(Role::Key), USER_ROLE);
        assert_eq!(role_id(Role::Text), USER_ROLE + 6);
        let names: std::collections::HashSet<_> = Role::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(names.len(), Role::ALL.len(), "every role has its own name");
        assert!(Role::ALL.iter().enumerate().all(|(i, r)| r.index() == i));
    }

    #[test]
    fn links_open_with_their_host_in_ascii() {
        assert_eq!(ascii_url("https://bücher.example/a b"), "https://xn--bcher-kva.example/a%20b");
        assert_eq!(ascii_url("mailto:someone@example.com"), "mailto:someone@example.com");
    }
}
