// SPDX-License-Identifier: GPL-3.0-or-later
//! `Agents` and `AgentItems`: the Agents page. `Agents` holds the lists, the
//! open thread or session and the setup as properties (JSON QML reads) and
//! takes the page's requests; `AgentItems` is the open view's transcript as
//! a list model, changed row by row so streaming text doesn't rebuild it.

use std::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QUrl, QVariant};

use crate::{
    agents::{self, Place, Row, UiDelta, UiUpdate},
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
        /// The open thread's approvals (JSON).
        #[qproperty(QString, asks)]
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
        type Agents = super::AgentsRust;

        /// The page is on screen (or left).
        #[qinvokable]
        fn page_visible(self: &Agents, visible: bool);
        #[qinvokable]
        fn start_host(self: &Agents);
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
        /// Sends a message to the open thread.
        #[qinvokable]
        fn send(self: &Agents, text: &QString);
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
        type AgentItems = super::AgentItemsRust;
    }

    // Inherited from QAbstractListModel.
    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginInsertRows"]
        unsafe fn begin_insert_rows(self: Pin<&mut AgentItems>, parent: &QModelIndex, first: i32, last: i32);
        #[inherit]
        #[cxx_name = "endInsertRows"]
        unsafe fn end_insert_rows(self: Pin<&mut AgentItems>);
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        unsafe fn begin_remove_rows(self: Pin<&mut AgentItems>, parent: &QModelIndex, first: i32, last: i32);
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        unsafe fn end_remove_rows(self: Pin<&mut AgentItems>);
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut AgentItems>);
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut AgentItems>);
        #[inherit]
        fn index(self: &AgentItems, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;

        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut AgentItems>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );
    }

    unsafe extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &AgentItems, index: &QModelIndex, role: i32) -> QVariant;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &AgentItems) -> QHash_i32_QByteArray;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &AgentItems, parent: &QModelIndex) -> i32;
        /// The row of an item's `seq` (as text), or -1.
        #[qinvokable]
        #[cxx_name = "rowOfSeq"]
        fn row_of_seq(self: &AgentItems, seq: &QString) -> i32;
    }

    impl cxx_qt::Threading for AgentItems {}
    impl cxx_qt::Initialize for AgentItems {}
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
    asks: QString,
    steps: QString,
    changes: QString,
    more: bool,
    loading: bool,
    busy: bool,
    export_file: QString,
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
        if differs(|v| &v.asks) {
            self.as_mut().set_asks(QString::from(&view.asks));
        }
        if differs(|v| &v.steps) {
            self.as_mut().set_steps(QString::from(&view.steps));
        }
        if differs(|v| &v.changes) {
            self.as_mut().set_changes(QString::from(&view.changes));
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
        self.as_mut().set_more(view.more);
        self.as_mut().set_loading(view.loading);
        self.as_mut().set_busy(view.busy);
        self.as_mut().rust_mut().last = Some(view);
    }

    pub fn page_visible(&self, visible: bool) {
        agents::page_visible(visible);
    }

    pub fn start_host(&self) {
        agents::start_host();
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

    pub fn send(&self, text: &QString) {
        agents::send(String::from(text));
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
}

// ---- The transcript ----

#[derive(Default)]
pub struct AgentItemsRust {
    count: i32,
    rows: Vec<Row>,
}

/// Model roles (Qt::UserRole + n).
const ROLES: &[&str] = &[
    "seq",
    "itemId",
    "kind",
    "text",
    "truncated",
    "from",
    "toolName",
    "toolTitle",
    "toolDetail",
    "toolStatus",
    "toolOutput",
    "streaming",
    "whenMs",
];
const USER_ROLE: i32 = 0x0100;

fn role(name: &str) -> i32 {
    USER_ROLE + ROLES.iter().position(|r| *r == name).map_or(0, |i| i as i32)
}

fn role_value(row: &Row, role: &str) -> QVariant {
    let text = |s: &str| QVariant::from(&QString::from(s));
    match role {
        // A session's seq doesn't fit a JS number safely.
        "seq" => text(&row.seq.to_string()),
        "itemId" => text(&row.item_id),
        "kind" => text(&row.kind),
        "text" => text(&row.text),
        "truncated" => QVariant::from(&row.truncated),
        "from" => text(&row.from),
        "toolName" => text(&row.tool_name),
        "toolTitle" => text(&row.tool_title),
        "toolDetail" => text(&row.tool_detail),
        "toolStatus" => text(&row.tool_status),
        "toolOutput" => text(&row.tool_output),
        "streaming" => QVariant::from(&row.streaming),
        "whenMs" => QVariant::from(&(row.when_ms as f64)),
        _ => QVariant::default(),
    }
}

impl cxx_qt::Initialize for qobject::AgentItems {
    fn initialize(self: Pin<&mut Self>) {
        let qt = self.qt_thread();
        agents::attach_ui(move || qt.queue(|object| object.apply_updates()).is_ok());
    }
}

impl qobject::AgentItems {
    fn apply_updates(mut self: Pin<&mut Self>) {
        for update in agents::take_ui() {
            match update {
                UiUpdate::Item(row) => self.as_mut().apply_row(*row),
                UiUpdate::Delta(delta) => self.as_mut().apply_delta(delta),
                UiUpdate::Reset => self.as_mut().reset(agents::open_rows()),
            }
        }
        let count = self.rows.len() as i32;
        self.set_count(count);
    }

    fn reset(mut self: Pin<&mut Self>, rows: Vec<Row>) {
        // SAFETY: begin/end are balanced and the rows change between them.
        unsafe {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().rows = rows;
            self.as_mut().end_reset_model();
        }
    }

    /// The same rule as the bridge's own copy (`agents::place`, `agents::overflow`).
    fn apply_row(mut self: Pin<&mut Self>, row: Row) {
        let root = QModelIndex::default();
        match agents::place(&self.rows, &row) {
            Place::Stale => {}
            Place::Replace(i) => {
                self.as_mut().rust_mut().rows[i] = row;
                let index = self.index(i as i32, 0, &root);
                self.as_mut().data_changed(&index, &index, &QList::default());
            }
            Place::Insert(i) => {
                let drop = agents::overflow(self.rows.len(), i);
                let r = i as i32;
                // SAFETY: i <= len; begin/end are balanced.
                unsafe {
                    self.as_mut().begin_insert_rows(&root, r, r);
                    self.as_mut().rust_mut().rows.insert(i, row);
                    self.as_mut().end_insert_rows();
                }
                if drop > 0 {
                    // SAFETY: the first `drop` rows exist; begin/end are balanced.
                    unsafe {
                        self.as_mut().begin_remove_rows(&root, 0, drop as i32 - 1);
                        self.as_mut().rust_mut().rows.drain(..drop);
                        self.as_mut().end_remove_rows();
                    }
                }
            }
        }
    }

    /// More text for a streaming row: only its text and `streaming` change.
    fn apply_delta(mut self: Pin<&mut Self>, delta: UiDelta) {
        let Ok(i) = self.rows.binary_search_by_key(&delta.seq, |r| r.seq) else { return };
        if agents::append(&mut self.as_mut().rust_mut().rows[i], delta.offset, &delta.text).is_none() {
            return;
        }
        let root = QModelIndex::default();
        let index = self.index(i as i32, 0, &root);
        let mut roles = QList::<i32>::default();
        roles.append(role("text"));
        roles.append(role("streaming"));
        roles.append(role("truncated"));
        self.as_mut().data_changed(&index, &index, &roles);
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let row = usize::try_from(index.row()).ok();
        let role = usize::try_from(role - USER_ROLE).ok().and_then(|r| ROLES.get(r));
        match (row.and_then(|r| self.rows.get(r)), role) {
            (Some(row), Some(role)) => role_value(row, role),
            _ => QVariant::default(),
        }
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (i, name) in ROLES.iter().enumerate() {
            roles.insert(USER_ROLE + i as i32, QByteArray::from(*name));
        }
        roles
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.rows.len() as i32
    }

    pub fn row_of_seq(&self, seq: &QString) -> i32 {
        let Ok(seq) = String::from(seq).parse::<u64>() else { return -1 };
        self.rows.binary_search_by_key(&seq, |r| r.seq).map_or(-1, |i| i as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_cover_the_row() {
        let row = Row {
            seq: (1 << 40) | 3,
            kind: "tool".into(),
            tool_status: "running".into(),
            streaming: true,
            when_ms: 1_795_000_000_000,
            ..Row::default()
        };
        let get = |name| role_value(&row, name);
        assert_eq!(get("seq").value::<QString>().map(String::from), Some(((1u64 << 40) | 3).to_string()));
        assert_eq!(get("toolStatus").value::<QString>().map(String::from).as_deref(), Some("running"));
        assert_eq!(get("streaming").value::<bool>(), Some(true));
        assert_eq!(get("whenMs").value::<f64>(), Some(1_795_000_000_000.0));
        assert_eq!(role("seq"), USER_ROLE);
        assert_eq!(role("text"), USER_ROLE + 3);
    }
}
