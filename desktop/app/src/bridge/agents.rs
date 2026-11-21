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
    agents::{self, ModelRow, Role, RoleValue, RowOp, UiUpdate, source},
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
        /// The Archived or Snoozed shelf's threads (JSON; `null` for the main list).
        #[qproperty(QString, shelf)]
        /// The list's search and its hits by thread (JSON).
        #[qproperty(QString, search)]
        /// "Bring in earlier sessions" (JSON, `{}` while it's closed).
        #[qproperty(QString, bring_in)]
        /// The diff (JSON, `{}` while it's closed).
        #[qproperty(QString, diff)]
        /// The rewind dialog (JSON, `{}` while it's closed).
        #[qproperty(QString, rewind)]
        /// The message to scroll to and light up: `{ gen, n, item }` (JSON).
        #[qproperty(QString, focus)]
        /// The open thread's source control: strip, panel, dialogs (JSON, `{}` for none).
        #[qproperty(QString, git)]
        /// The Pull Requests view (JSON, `{}` while it's closed).
        #[qproperty(QString, prs)]
        /// Setup's Source control and the new thread's worktree (JSON).
        #[qproperty(QString, git_setup)]
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
        /// Deletes a thread; `remove_worktree`: its worktree too.
        #[qinvokable]
        fn delete_thread_with(self: &Agents, id: &QString, remove_worktree: bool);
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
        /// Add image's Paste: the clipboard's image, or a line saying why not.
        #[qinvokable]
        fn paste_from_menu(self: &Agents);
        /// "Take a photo with <phone>" (`device`: its id, as `cameras` lists it).
        #[qinvokable]
        fn take_photo(self: &Agents, device: &QString);
        /// The strip's Cancel while a photo is awaited.
        #[qinvokable]
        fn cancel_photo(self: &Agents);
        /// "Continue on <phone>" (`device`: its id, as `handoff` lists it).
        #[qinvokable]
        fn handoff(self: &Agents, device: &QString);
        /// The phone beside the PC (`device`: its id, as `beside` names it)
        /// shows `pane` of the open thread: "changes", "plan" or "requests".
        #[qinvokable]
        fn beside(self: &Agents, device: &QString, pane: &QString);
        /// "Open in browser": the open thread's dev server on `port`.
        #[qinvokable]
        fn open_server(self: &Agents, port: i32);
        /// "Open on <phone>" (`device`: its id, as `server` lists it).
        #[qinvokable]
        fn open_server_on(self: &Agents, port: i32, device: &QString);
        /// The first message on screen ("" while the newest are shown).
        #[qinvokable]
        fn reading_at(self: &Agents, item_id: &QString);
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
        /// "Tell me when a limit resets".
        #[qinvokable]
        fn set_limit_alarm(self: &Agents, on: bool);
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
        /// The list shows "main", "archived" or "snoozed".
        #[qinvokable]
        fn show_shelf(self: &Agents, shelf: &QString);
        /// Marks threads (a JSON list of ids, at most 50): "pin", "unpin",
        /// "archive", "unarchive", "unread", "read", "snooze" (until
        /// `until_ms`) or "unsnooze".
        #[qinvokable]
        fn mark(self: &Agents, ids: &QString, what: &QString, until_ms: f64);
        #[qinvokable]
        fn rename(self: &Agents, id: &QString, title: &QString);
        /// Searches threads 150 ms after the last change.
        #[qinvokable]
        fn search_threads(self: &Agents, query: &QString, archived: bool);
        /// Opens a search hit's thread at its message (`seq` "0": the newest).
        #[qinvokable]
        fn open_hit(self: &Agents, thread: &QString, seq: &QString);
        /// "Bring in earlier sessions" opens: the host looks for sessions.
        #[qinvokable]
        fn import_open(self: &Agents);
        #[qinvokable]
        fn import_close(self: &Agents);
        #[qinvokable]
        fn import_query(self: &Agents, query: &QString);
        /// "all", "claude" or "codex".
        #[qinvokable]
        fn import_agent(self: &Agents, agent: &QString);
        #[qinvokable]
        fn import_more(self: &Agents);
        #[qinvokable]
        fn import_pick(self: &Agents, id: &QString);
        /// Picks every session of a folder, or leaves them.
        #[qinvokable]
        fn import_pick_group(self: &Agents, key: &QString);
        #[qinvokable]
        fn import_clear(self: &Agents);
        /// Brings in the sessions picked, or only `only`.
        #[qinvokable]
        fn import_start(self: &Agents, only: &QString);
        #[qinvokable]
        fn import_cancel(self: &Agents);
        /// Looks at a session before bringing it in.
        #[qinvokable]
        fn preview_open(self: &Agents, id: &QString);
        #[qinvokable]
        fn preview_close(self: &Agents);
        /// Opens what a turn changed, at `path` ("" for its first file).
        #[qinvokable]
        fn diff_open(self: &Agents, turn: i32, path: &QString);
        /// "turn" or "thread".
        #[qinvokable]
        fn diff_scope(self: &Agents, scope: &QString);
        #[qinvokable]
        fn diff_select(self: &Agents, index: i32);
        /// The next (1) or previous (-1) file.
        #[qinvokable]
        fn diff_step(self: &Agents, by: i32);
        #[qinvokable]
        fn diff_close(self: &Agents);
        /// Opens the dialog that asks before rewinding to before `turn`.
        #[qinvokable]
        fn rewind_open(self: &Agents, turn: i32);
        /// "both", "conversation" or "files".
        #[qinvokable]
        fn rewind_choose(self: &Agents, choice: &QString);
        /// "Also undo the changes made after the agent finished".
        #[qinvokable]
        fn rewind_later(self: &Agents, on: bool);
        /// The Rewind button gained or lost the keyboard.
        #[qinvokable]
        fn rewind_focus(self: &Agents, focused: bool);
        /// Rewind: "click", or "key" (counts only on the button reached with Tab).
        #[qinvokable]
        fn rewind_apply(self: &Agents, how: &QString);
        #[qinvokable]
        fn rewind_close(self: &Agents);
        /// The conversation back to before `turn`, its message in the composer.
        #[qinvokable]
        fn edit_from_here(self: &Agents, turn: i32);
        /// Undoes the open thread's last rewind.
        #[qinvokable]
        fn undo_rewind(self: &Agents);
        /// A new thread from the open one, through `turn`; it opens.
        #[qinvokable]
        fn fork_turn(self: &Agents, turn: i32);
        /// A folder's "Keep a restore point for each turn".
        #[qinvokable]
        fn set_folder_checkpoints(self: &Agents, id: &QString, on: bool);
        /// Reads what the host keeps, for Setup.
        #[qinvokable]
        fn store_info(self: &Agents);
        /// Starts a thread in a new worktree on `branch`, from `base` ("" for
        /// the folder's current commit).
        #[qinvokable]
        fn start_worktree(
            self: &Agents,
            agent: &QString,
            folder_id: &QString,
            prompt: &QString,
            mode: &QString,
            branch: &QString,
            base: &QString,
        );
        /// A worktree branch for a new thread from its first words.
        #[qinvokable]
        fn worktree_branch(self: &Agents, words: &QString) -> QString;
        /// Whether `name` may be a new branch (the host checks again).
        #[qinvokable]
        fn branch_name_ok(self: &Agents, name: &QString) -> bool;
        /// The source-control panel opens (it reads, fetches) or closes.
        #[qinvokable]
        fn git_panel(self: &Agents, open: bool);
        /// Fetches and reads the status and pull request again.
        #[qinvokable]
        fn git_refresh(self: &Agents);
        #[qinvokable]
        fn git_tick(self: &Agents, path: &QString, on: bool);
        #[qinvokable]
        fn git_tick_all(self: &Agents, on: bool);
        /// A field changed: "message", "title" or "body".
        #[qinvokable]
        fn git_field(self: &Agents, which: &QString, text: &QString);
        /// "Use the thread's title" in the commit message.
        #[qinvokable]
        fn git_use_title(self: &Agents);
        /// "Use <agent>'s message": "message" or "pr".
        #[qinvokable]
        fn git_use_offer(self: &Agents, which: &QString);
        /// "Ask <agent> to write it": "commit" or "pr".
        #[qinvokable]
        fn git_ask(self: &Agents, purpose: &QString);
        /// Commits the ticked files (`all`: every file, past the list's 2,000).
        #[qinvokable]
        fn git_commit(self: &Agents, all: bool);
        /// "Reload" after the files changed.
        #[qinvokable]
        fn git_reload(self: &Agents);
        /// Stops the running commit or push.
        #[qinvokable]
        fn git_stop(self: &Agents);
        /// Pushes, to `remote` ("" for the branch's own).
        #[qinvokable]
        fn git_push(self: &Agents, remote: &QString);
        #[qinvokable]
        fn git_branch_menu(self: &Agents, open: bool);
        #[qinvokable]
        fn git_branch_query(self: &Agents, query: &QString);
        /// Switches (`create`: a new branch; `carry`: with the changes, confirmed).
        #[qinvokable]
        fn git_switch(self: &Agents, branch: &QString, create: bool, carry: bool);
        /// "New branch first…": the push follows the new branch.
        #[qinvokable]
        fn git_new_branch_then_push(self: &Agents);
        /// The pull request dialog opens or closes.
        #[qinvokable]
        fn pr_form(self: &Agents, open: bool);
        /// Opens a pull request (after a push when the branch isn't pushed).
        #[qinvokable]
        fn pr_create(self: &Agents, title: &QString, body: &QString, base: &QString, draft: bool);
        #[qinvokable]
        fn pr_merge_open(self: &Agents);
        /// "squash", "merge" or "rebase".
        #[qinvokable]
        fn pr_merge_method(self: &Agents, method: &QString);
        /// "I've looked".
        #[qinvokable]
        fn pr_merge_looked(self: &Agents, on: bool);
        /// The Merge button gained or lost the keyboard.
        #[qinvokable]
        fn pr_merge_focus(self: &Agents, focused: bool);
        /// Merge: "click", or "key" (counts only on the button reached with Tab).
        #[qinvokable]
        fn pr_merge(self: &Agents, how: &QString);
        #[qinvokable]
        fn pr_merge_close(self: &Agents);
        /// Mark ready.
        #[qinvokable]
        fn pr_ready(self: &Agents);
        /// Opens a pull request's or check's page, on a signed-in GitHub host only.
        #[qinvokable]
        fn open_pr_url(self: &Agents, url: &QString);
        /// The Pull Requests list: "mine", "review" or "folder" (`folder_id`, "" for the open thread's).
        #[qinvokable]
        fn prs_open(self: &Agents, show: &QString, folder_id: &QString);
        #[qinvokable]
        fn prs_close(self: &Agents);
        #[qinvokable]
        fn prs_refresh(self: &Agents);
        /// Setup's Source control: the GitHub CLI (`refresh`: "Check again") and the worktrees.
        #[qinvokable]
        fn git_setup_read(self: &Agents, refresh: bool);
        /// A folder's "Keep in step with the remote".
        #[qinvokable]
        fn set_follow_remote(self: &Agents, id: &QString, on: bool);
        /// Removes a worktree's folder (its branch stays), or forgets a missing one.
        #[qinvokable]
        fn worktree_remove(self: &Agents, id: &QString, forget: bool);
        /// Removes the open thread's worktree.
        #[qinvokable]
        fn worktree_remove_open(self: &Agents);
        /// The branches a new worktree in `folder_id` may start from.
        #[qinvokable]
        fn read_bases(self: &Agents, folder_id: &QString);
        /// The new-thread form drops a failed worktree's lines.
        #[qinvokable]
        fn creating_clear(self: &Agents);
        /// A quote chip of rows `first`..`last` of the diff's file.
        #[qinvokable]
        fn quote_add(self: &Agents, first: i32, last: i32);
        #[qinvokable]
        fn quote_remove(self: &Agents, index: i32);
        /// Opens what isn't committed in the open thread's folder, at `path`.
        #[qinvokable]
        fn diff_uncommitted(self: &Agents, path: &QString);
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
        /// The first row of an item, or -1.
        #[qinvokable]
        #[cxx_name = "rowOfItem"]
        fn row_of_item(self: &AgentTimeline, item_id: &QString) -> i32;
        /// The item of the row at `row` or the first one after it that has
        /// one ("" for none).
        #[qinvokable]
        #[cxx_name = "itemIdAt"]
        fn item_id_at(self: &AgentTimeline, row: i32) -> QString;
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
    shelf: QString,
    search: QString,
    bring_in: QString,
    diff: QString,
    rewind: QString,
    focus: QString,
    git: QString,
    prs: QString,
    git_setup: QString,
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
        if differs(|v| &v.shelf) {
            self.as_mut().set_shelf(QString::from(&view.shelf));
        }
        if differs(|v| &v.search) {
            self.as_mut().set_search(QString::from(&view.search));
        }
        if differs(|v| &v.import) {
            self.as_mut().set_bring_in(QString::from(&view.import));
        }
        if differs(|v| &v.diff) {
            self.as_mut().set_diff(QString::from(&view.diff));
        }
        if differs(|v| &v.rewind) {
            self.as_mut().set_rewind(QString::from(&view.rewind));
        }
        if differs(|v| &v.focus) {
            self.as_mut().set_focus(QString::from(&view.focus));
        }
        if differs(|v| &v.git) {
            self.as_mut().set_git(QString::from(&view.git));
        }
        if differs(|v| &v.prs) {
            self.as_mut().set_prs(QString::from(&view.prs));
        }
        if differs(|v| &v.git_setup) {
            self.as_mut().set_git_setup(QString::from(&view.git_setup));
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
        agents::start_thread(String::from(agent), String::from(folder_id), prompt, String::from(mode), None);
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
        agents::delete_thread(String::from(id), false);
    }

    pub fn delete_thread_with(&self, id: &QString, remove_worktree: bool) {
        agents::delete_thread(String::from(id), remove_worktree);
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

    pub fn paste_from_menu(&self) {
        agents::paste_from_menu();
    }

    pub fn take_photo(&self, device: &QString) {
        agents::take_photo(String::from(device));
    }

    pub fn cancel_photo(&self) {
        agents::cancel_photo();
    }

    pub fn handoff(&self, device: &QString) {
        agents::handoff_to(String::from(device));
    }

    pub fn beside(&self, device: &QString, pane: &QString) {
        agents::switch_beside(String::from(device), String::from(pane));
    }

    pub fn open_server(&self, port: i32) {
        agents::open_server(port);
    }

    pub fn open_server_on(&self, port: i32, device: &QString) {
        agents::open_server_on(port, String::from(device));
    }

    pub fn reading_at(&self, item_id: &QString) {
        agents::reading_at(String::from(item_id));
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

    pub fn set_limit_alarm(&self, on: bool) {
        agents::set_limit_alarm(on);
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

    pub fn show_shelf(&self, shelf: &QString) {
        agents::show_shelf(&String::from(shelf));
    }

    pub fn mark(&self, ids: &QString, what: &QString, until_ms: f64) {
        let ids: Vec<String> = serde_json::from_str(&String::from(ids)).unwrap_or_default();
        // A time from the page: whole milliseconds, never negative.
        let until = if until_ms.is_finite() && until_ms > 0.0 { until_ms as u64 } else { 0 };
        agents::mark(ids, &String::from(what), until);
    }

    pub fn rename(&self, id: &QString, title: &QString) {
        agents::rename(String::from(id), String::from(title));
    }

    pub fn search_threads(&self, query: &QString, archived: bool) {
        agents::search(String::from(query), archived);
    }

    pub fn open_hit(&self, thread: &QString, seq: &QString) {
        let seq = String::from(seq).parse().unwrap_or(0);
        agents::open_hit(String::from(thread), seq);
    }

    pub fn import_open(&self) {
        agents::import_open();
    }

    pub fn import_close(&self) {
        agents::import_close();
    }

    pub fn import_query(&self, query: &QString) {
        agents::import_query(String::from(query));
    }

    pub fn import_agent(&self, agent: &QString) {
        agents::import_agent(&String::from(agent));
    }

    pub fn import_more(&self) {
        agents::import_more();
    }

    pub fn import_pick(&self, id: &QString) {
        agents::import_pick(String::from(id));
    }

    pub fn import_pick_group(&self, key: &QString) {
        agents::import_pick_group(String::from(key));
    }

    pub fn import_clear(&self) {
        agents::import_clear();
    }

    pub fn import_start(&self, only: &QString) {
        agents::import_start(String::from(only));
    }

    pub fn import_cancel(&self) {
        agents::import_cancel();
    }

    pub fn preview_open(&self, id: &QString) {
        agents::preview_open(String::from(id));
    }

    pub fn preview_close(&self) {
        agents::preview_close();
    }

    pub fn diff_open(&self, turn: i32, path: &QString) {
        if let Ok(turn) = u32::try_from(turn) {
            agents::diff_open(turn, String::from(path));
        }
    }

    pub fn diff_scope(&self, scope: &QString) {
        agents::diff_scope(&String::from(scope));
    }

    pub fn diff_select(&self, index: i32) {
        if let Ok(index) = usize::try_from(index) {
            agents::diff_select(index);
        }
    }

    pub fn diff_step(&self, by: i32) {
        agents::diff_step(by);
    }

    pub fn diff_close(&self) {
        agents::diff_close();
    }

    pub fn rewind_open(&self, turn: i32) {
        if let Ok(turn) = u32::try_from(turn) {
            agents::rewind_open(turn);
        }
    }

    pub fn rewind_choose(&self, choice: &QString) {
        agents::rewind_choose(&String::from(choice));
    }

    pub fn rewind_later(&self, on: bool) {
        agents::rewind_later(on);
    }

    pub fn rewind_focus(&self, focused: bool) {
        agents::rewind_focus(focused);
    }

    pub fn rewind_apply(&self, how: &QString) {
        agents::rewind_apply(&String::from(how));
    }

    pub fn rewind_close(&self) {
        agents::rewind_close();
    }

    pub fn edit_from_here(&self, turn: i32) {
        if let Ok(turn) = u32::try_from(turn) {
            agents::edit_from_here(turn);
        }
    }

    pub fn undo_rewind(&self) {
        agents::undo_rewind();
    }

    pub fn fork_turn(&self, turn: i32) {
        if let Ok(turn) = u32::try_from(turn) {
            agents::fork_turn(turn);
        }
    }

    pub fn set_folder_checkpoints(&self, id: &QString, on: bool) {
        agents::set_folder_checkpoints(String::from(id), on);
    }

    pub fn store_info(&self) {
        agents::store_info();
    }

    pub fn start_worktree(
        &self,
        agent: &QString,
        folder_id: &QString,
        prompt: &QString,
        mode: &QString,
        branch: &QString,
        base: &QString,
    ) {
        let prompt = String::from(prompt);
        if prompt.trim().is_empty() {
            return;
        }
        let worktree = Some((String::from(branch).trim().to_owned(), String::from(base)));
        agents::start_thread(
            String::from(agent),
            String::from(folder_id),
            prompt,
            String::from(mode),
            worktree,
        );
    }

    pub fn worktree_branch(&self, words: &QString) -> QString {
        QString::from(&source::worktree_branch(&String::from(words)))
    }

    pub fn branch_name_ok(&self, name: &QString) -> bool {
        source::branch_name_ok(&String::from(name))
    }

    pub fn git_panel(&self, open: bool) {
        source::panel(open);
    }

    pub fn git_refresh(&self) {
        source::refresh();
    }

    pub fn git_tick(&self, path: &QString, on: bool) {
        source::tick(String::from(path), on);
    }

    pub fn git_tick_all(&self, on: bool) {
        source::tick_all(on);
    }

    pub fn git_field(&self, which: &QString, text: &QString) {
        source::field_edited(&String::from(which), String::from(text));
    }

    pub fn git_use_title(&self) {
        source::use_title();
    }

    pub fn git_use_offer(&self, which: &QString) {
        source::use_offer(&String::from(which));
    }

    pub fn git_ask(&self, purpose: &QString) {
        source::ask(&String::from(purpose));
    }

    pub fn git_commit(&self, all: bool) {
        source::commit(all);
    }

    pub fn git_reload(&self) {
        source::reload();
    }

    pub fn git_stop(&self) {
        source::stop();
    }

    pub fn git_push(&self, remote: &QString) {
        source::push(String::from(remote));
    }

    pub fn git_branch_menu(&self, open: bool) {
        source::branch_menu(open);
    }

    pub fn git_branch_query(&self, query: &QString) {
        source::branch_query(String::from(query));
    }

    pub fn git_switch(&self, branch: &QString, create: bool, carry: bool) {
        source::switch(String::from(branch).trim().to_owned(), create, carry);
    }

    pub fn git_new_branch_then_push(&self) {
        source::new_branch_then_push();
    }

    pub fn pr_form(&self, open: bool) {
        source::pr_form(open);
    }

    pub fn pr_create(&self, title: &QString, body: &QString, base: &QString, draft: bool) {
        source::pr_create(String::from(title), String::from(body), String::from(base), draft);
    }

    pub fn pr_merge_open(&self) {
        source::merge_open();
    }

    pub fn pr_merge_method(&self, method: &QString) {
        source::merge_method(&String::from(method));
    }

    pub fn pr_merge_looked(&self, on: bool) {
        source::merge_looked(on);
    }

    pub fn pr_merge_focus(&self, focused: bool) {
        source::merge_focus(focused);
    }

    pub fn pr_merge(&self, how: &QString) {
        source::merge(&String::from(how));
    }

    pub fn pr_merge_close(&self) {
        source::merge_close();
    }

    pub fn pr_ready(&self) {
        source::ready();
    }

    pub fn open_pr_url(&self, url: &QString) {
        source::open_url(String::from(url));
    }

    pub fn prs_open(&self, show: &QString, folder_id: &QString) {
        source::prs_open(&String::from(show), String::from(folder_id));
    }

    pub fn prs_close(&self) {
        source::prs_close();
    }

    pub fn prs_refresh(&self) {
        source::prs_refresh();
    }

    pub fn git_setup_read(&self, refresh: bool) {
        source::setup_read(refresh);
    }

    pub fn set_follow_remote(&self, id: &QString, on: bool) {
        source::set_follow_remote(String::from(id), on);
    }

    pub fn worktree_remove(&self, id: &QString, forget: bool) {
        source::worktree_remove(String::from(id), forget);
    }

    pub fn worktree_remove_open(&self) {
        source::worktree_remove_open();
    }

    pub fn read_bases(&self, folder_id: &QString) {
        source::read_bases(String::from(folder_id));
    }

    pub fn creating_clear(&self) {
        source::creating_clear();
    }

    pub fn quote_add(&self, first: i32, last: i32) {
        if let (Ok(first), Ok(last)) = (usize::try_from(first), usize::try_from(last)) {
            source::quote_add(first, last);
        }
    }

    pub fn quote_remove(&self, index: i32) {
        if let Ok(index) = usize::try_from(index) {
            source::quote_remove(index);
        }
    }

    pub fn diff_uncommitted(&self, path: &QString) {
        agents::diff_uncommitted(String::from(path));
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

/// The item of row `row`, or of the first row after it that has one (a
/// fold line or a group belongs to none).
fn first_item(rows: &[Arc<ModelRow>], row: usize) -> Option<&str> {
    rows.iter().skip(row).map(|r| r.text(Role::ItemId)).find(|id| !id.is_empty())
}

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

    pub fn row_of_item(&self, item_id: &QString) -> i32 {
        let item_id = String::from(item_id);
        self.rows.iter().position(|r| r.text(Role::ItemId) == item_id).map_or(-1, |i| i as i32)
    }

    pub fn item_id_at(&self, row: i32) -> QString {
        let Ok(row) = usize::try_from(row) else { return QString::default() };
        first_item(&self.rows, row).map(QString::from).unwrap_or_default()
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
                label: ToolLabel {
                    class: ToolClass::Command,
                    object: "cargo test".into(),
                    more: 0,
                    toks: Default::default(),
                },
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
            acts: Default::default(),
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
    fn a_hand_off_opens_at_the_first_message_on_screen() {
        let user = |item_id: &str| Row {
            key: format!("i:{item_id}"),
            kind: RowKind::User {
                text: "hi".into(),
                from: String::new(),
                deny_of: false,
                via: Default::default(),
            },
            turn: 1,
            depth: 0,
            item_id: item_id.into(),
            seq: 1,
            acts: Default::default(),
        };
        let rows: Vec<Arc<ModelRow>> = [user(""), user("u1"), user("u2")]
            .iter()
            .map(|r| {
                Arc::new(agents::model_row(r, agents::Extra::default(), "", &agents::CodePalette::default()))
            })
            .collect();
        assert_eq!(first_item(&rows, 0), Some("u1"), "a row of no item gives the next one's");
        assert_eq!(first_item(&rows, 2), Some("u2"));
        assert_eq!(first_item(&rows, 3), None);
    }

    #[test]
    fn links_open_with_their_host_in_ascii() {
        assert_eq!(ascii_url("https://bücher.example/a b"), "https://xn--bcher-kva.example/a%20b");
        assert_eq!(ascii_url("mailto:someone@example.com"), "mailto:someone@example.com");
    }
}
