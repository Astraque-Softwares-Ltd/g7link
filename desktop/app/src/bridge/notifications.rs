// SPDX-License-Identifier: GPL-3.0-or-later
//! `NotificationList`: phone notifications (newest first) as a Qt list
//! model, plus the commands QML runs on them.

use crate::i18n::tr;
use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    path::PathBuf,
    pin::Pin,
};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QList, QModelIndex, QString, QVariant};
use g7link_core::{DeviceId, Notification};

use super::{Edit, diff};
use crate::{
    core_host, icons,
    state::{AppRule, AppState, Changes, NotificationView, SentReply},
};

#[cxx_qt::bridge]
pub mod qobject {
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
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, count)]
        /// Keep a day of history.
        #[qproperty(bool, history_enabled, cxx_name = "historyEnabled", READ, NOTIFY)]
        /// Apps that sent notifications, as JSON sorted by name:
        /// [{ "app", "name", "icon", "rule": "show" | "quiet" | "hidden" }].
        #[qproperty(QString, apps, READ, NOTIFY)]
        /// Non-empty when a recent dismissal can be undone in the feed.
        #[qproperty(QString, undo_label, cxx_name = "undoLabel", READ, NOTIFY)]
        /// The phone whose notifications the rows hold ("": every phone's).
        #[qproperty(QString, device)]
        /// The filter: "" (all), "messages", "calls" or "apps".
        #[qproperty(QString, category)]
        /// How many notifications that phone has under each filter, as JSON:
        /// { "all", "messages", "calls", "apps" }.
        #[qproperty(QString, category_counts, cxx_name = "categoryCounts")]
        /// How many app groups (and Live Updates) the rows hold.
        #[qproperty(i32, group_total, cxx_name = "groupTotal")]
        type NotificationList = super::NotificationListRust;
    }

    // Inherited from QAbstractListModel.
    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginInsertRows"]
        unsafe fn begin_insert_rows(
            self: Pin<&mut NotificationList>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        #[inherit]
        #[cxx_name = "endInsertRows"]
        unsafe fn end_insert_rows(self: Pin<&mut NotificationList>);
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        unsafe fn begin_remove_rows(
            self: Pin<&mut NotificationList>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        unsafe fn end_remove_rows(self: Pin<&mut NotificationList>);
        #[inherit]
        fn index(self: &NotificationList, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;

        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut NotificationList>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );
    }

    unsafe extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &NotificationList, index: &QModelIndex, role: i32) -> QVariant;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &NotificationList) -> QHash_i32_QByteArray;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &NotificationList, parent: &QModelIndex) -> i32;

        /// How many notifications a device has here.
        #[qinvokable]
        #[cxx_name = "countFor"]
        fn count_for(self: &NotificationList, device: &QString) -> i32;
        /// Dismisses it here and on the phone.
        #[qinvokable]
        fn dismiss(self: &NotificationList, device: &QString, key: &QString);
        /// Dismisses all notifications from one app on a device (here and on the phone).
        #[qinvokable]
        #[cxx_name = "dismissGroup"]
        fn dismiss_group(self: &NotificationList, device: &QString, app: &QString);
        /// Dismisses all of a device's notifications.
        #[qinvokable]
        #[cxx_name = "dismissAll"]
        fn dismiss_all(self: &NotificationList, device: &QString);
        /// Toggles whether an app's notification group is expanded in the feed.
        #[qinvokable]
        #[cxx_name = "toggleGroup"]
        fn toggle_group(self: &NotificationList, app: &QString);
        /// Shows all of an expanded app group's notifications, or only the
        /// latest few.
        #[qinvokable]
        #[cxx_name = "showAll"]
        fn show_all(self: Pin<&mut NotificationList>, app: &QString, on: bool);
        /// Restores the most recently dismissed notification or group to the PC feed.
        #[qinvokable]
        #[cxx_name = "undoDismiss"]
        fn undo_dismiss(self: &NotificationList);
        /// Hides the undo banner.
        #[qinvokable]
        #[cxx_name = "clearUndo"]
        fn clear_undo(self: &NotificationList);
        /// Runs one of its actions on the phone.
        #[qinvokable]
        #[cxx_name = "runAction"]
        fn run_action(self: &NotificationList, device: &QString, key: &QString, action: &QString);
        /// Sends a reply through its reply action.
        #[qinvokable]
        fn reply(self: &NotificationList, device: &QString, key: &QString, action: &QString, text: &QString);
        /// "show", "quiet" (no pop-ups) or "hidden".
        #[qinvokable]
        #[cxx_name = "appRule"]
        fn app_rule(self: &NotificationList, app: &QString) -> QString;
        #[qinvokable]
        #[cxx_name = "setAppRule"]
        fn set_app_rule(self: &NotificationList, app: &QString, rule: &QString);
        #[qinvokable]
        #[cxx_name = "setHistoryEnabled"]
        fn set_history_enabled_by_user(self: &NotificationList, on: bool);
        #[qinvokable]
        #[cxx_name = "clearHistory"]
        fn clear_history(self: &NotificationList);
        /// Copies a detected one-time code to the PC clipboard without syncing it back.
        #[qinvokable]
        #[cxx_name = "copyCode"]
        fn copy_code(self: &NotificationList, code: &QString);
    }

    impl cxx_qt::Threading for NotificationList {}
    impl cxx_qt::Initialize for NotificationList {}
}

/// What a row in the feed is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
enum Kind {
    /// An app's notifications, as one row: its icon, name, count and newest.
    Group,
    /// A notification: in an opened group, or a Live Update on its own.
    #[default]
    Item,
    /// Under an opened group: show all of it, or clear it.
    More,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Group => "group",
            Kind::Item => "item",
            Kind::More => "more",
        }
    }
}

/// The feed's filters. Messages: chat apps and texts; Calls: the phone app;
/// Apps: everything else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Category {
    #[default]
    All,
    Messages,
    Calls,
    Apps,
}

impl Category {
    fn named(name: &str) -> Category {
        match name {
            "messages" => Category::Messages,
            "calls" => Category::Calls,
            "apps" => Category::Apps,
            _ => Category::All,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Category::All => "",
            Category::Messages => "messages",
            Category::Calls => "calls",
            Category::Apps => "apps",
        }
    }
}

/// Chat and texting apps, by package, for when their notifications don't
/// say so themselves (a conversation, or a reply box).
const MESSAGING_APPS: &[&str] = &[
    "ch.threema.app",
    "com.android.messaging",
    "com.android.mms",
    "com.beeper.android",
    "com.discord",
    "com.facebook.mlite",
    "com.facebook.orca",
    "com.google.android.apps.dynamite",
    "com.google.android.apps.messaging",
    "com.kakao.talk",
    "com.microsoft.teams",
    "com.samsung.android.messaging",
    "com.skype.raider",
    "com.Slack",
    "com.snapchat.android",
    "com.tencent.mm",
    "com.viber.voip",
    "com.whatsapp",
    "com.whatsapp.w4b",
    "im.vector.app",
    "jp.naver.line.android",
    "org.telegram.messenger",
    "org.telegram.messenger.web",
    "org.thoughtcrime.securesms",
];

/// Phone (dialer and in-call) apps, by package.
const PHONE_APPS: &[&str] = &["com.android.phone", "com.android.server.telecom"];

/// Which filter an app's notifications are under: the phone app's under
/// Calls; chat apps, texting apps and the phone's SMS app under Messages;
/// the rest under Apps.
fn category_of(app: &str, members: &[&NotificationView], sms_app: Option<&str>) -> Category {
    if PHONE_APPS.contains(&app) || app.contains(".dialer") || app.contains("incallui") {
        return Category::Calls;
    }
    let chat = members
        .iter()
        .any(|v| v.notification.conversation.is_some() || v.notification.actions.iter().any(|a| a.reply));
    if chat || MESSAGING_APPS.contains(&app) || sms_app == Some(app) {
        Category::Messages
    } else {
        Category::Apps
    }
}

/// A row: the notification plus what the UI needs to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    kind: Kind,
    category: Category,
    view: NotificationView,
    device_name: String,
    /// A file URL, or "" (the UI then shows a glyph).
    icon_url: String,
    /// The notification's picture as a file URL, or "".
    image_url: String,
    /// Buttons other than the reply, as JSON: [{ "id", "title" }].
    actions: String,
    /// Replies sent from this PC, as JSON: [{ "text", "pending" }].
    replies: String,
    /// A one-time code in the notification, or "".
    otp_code: String,
    /// A Live Update's progress as JSON, or "".
    live: String,
    /// Notifications in this `(device, app)` group (`1` for a Live Update).
    group_count: i32,
    /// Where an item is in its opened group, from `1`; on a More row, how
    /// many of the group show; `0` on a group row and a Live Update.
    group_index: i32,
    /// Whether this `(device, app)` group is opened.
    group_expanded: bool,
    /// The group's older notifications, in brief.
    group_summary: String,
    /// Which app group (or Live Update) of the list this row belongs to, from `0`.
    group_ordinal: i32,
    /// An opened group shows all its notifications, not only the latest few.
    showing_all: bool,
}

/// Where a row is in the list and its `(device, app)` group.
#[derive(Debug, Default)]
struct Group {
    kind: Kind,
    category: Category,
    count: i32,
    index: i32,
    expanded: bool,
    summary: String,
    ordinal: i32,
    showing_all: bool,
}

impl Row {
    /// Works out what QML reads from the row once, not on every read.
    fn new(
        view: NotificationView,
        device_name: String,
        icon_url: String,
        image_url: String,
        replies: &[SentReply],
        group: Group,
    ) -> Self {
        let n = &view.notification;
        Self {
            actions: actions_json(n),
            replies: replies_json(replies),
            otp_code: crate::notifications::notification_otp(n).unwrap_or_default(),
            live: live_json(n),
            kind: group.kind,
            category: group.category,
            view,
            device_name,
            icon_url,
            image_url,
            group_count: group.count,
            group_index: group.index,
            group_expanded: group.expanded,
            group_summary: group.summary,
            group_ordinal: group.ordinal,
            showing_all: group.showing_all,
        }
    }
}

fn actions_json(n: &Notification) -> String {
    let mut actions = Vec::new();
    for a in n.actions.iter().filter(|a| !a.reply) {
        actions.push(serde_json::json!({ "id": a.id, "title": a.title }));
    }
    serde_json::Value::from(actions).to_string()
}

fn replies_json(replies: &[SentReply]) -> String {
    let mut out = Vec::with_capacity(replies.len());
    for r in replies {
        out.push(serde_json::json!({ "text": r.text, "pending": r.pending }));
    }
    serde_json::Value::from(out).to_string()
}

fn live_json(n: &Notification) -> String {
    let Some(live) = &n.live else { return String::new() };
    serde_json::json!({
        "progress": live.progress.unwrap_or(0),
        "max": live.max.unwrap_or(0),
        "indeterminate": live.indeterminate,
        "chip": live.chip.as_deref().unwrap_or_default(),
        "segments": live.segments.iter().map(|s| serde_json::json!({
            "length": s.length,
            "color": s.color.map(|c| format!("#{:06X}", c & 0x00FF_FFFF)).unwrap_or_default(),
        })).collect::<Vec<_>>(),
        "points": live.points.iter().map(|p| serde_json::json!({
            "position": p.position,
            "color": p.color.map(|c| format!("#{:06X}", c & 0x00FF_FFFF)).unwrap_or_default(),
        })).collect::<Vec<_>>(),
        "chronometer": live.chronometer,
        "countdown": live.countdown,
        "when": n.when,
    })
    .to_string()
}

#[derive(Default)]
pub struct NotificationListRust {
    count: i32,
    history_enabled: bool,
    apps: QString,
    undo_label: QString,
    device: QString,
    category: QString,
    category_counts: QString,
    group_total: i32,
    rows: Vec<Row>,
    /// Each phone's notifications here.
    per_device: HashMap<DeviceId, i32>,
    /// Opened groups showing all their notifications, by app.
    show_all: HashSet<String>,
}

/// Model roles (Qt::UserRole + n).
const ROLES: &[&str] = &[
    "kind",
    "category",
    "deviceId",
    "deviceName",
    "key",
    "app",
    "appName",
    "title",
    "text",
    "sub",
    "when",
    "iconUrl",
    "imageUrl",
    "actions",
    "replyAction",
    "replyLabel",
    "silent",
    "ongoing",
    "replies",
    "otpCode",
    "live",
    "groupCount",
    "groupIndex",
    "groupExpanded",
    "groupSummary",
    "groupOrdinal",
    "showingAll",
];
const USER_ROLE: i32 = 0x0100;

fn role_value(row: &Row, role: &str) -> QVariant {
    let text = |s: &str| QVariant::from(&QString::from(s));
    let n = &row.view.notification;
    let reply = n.actions.iter().find(|a| a.reply);
    match role {
        "kind" => text(row.kind.name()),
        "category" => text(row.category.name()),
        "deviceId" => text(&row.view.device.to_string()),
        "deviceName" => text(&row.device_name),
        "key" => text(&n.key),
        "app" => text(&n.app),
        "appName" => text(&n.app_name),
        "title" => text(n.title.as_deref().unwrap_or_default()),
        "text" => text(n.text.as_deref().unwrap_or_default()),
        "sub" => text(n.sub.as_deref().unwrap_or_default()),
        "when" => QVariant::from(&(n.when as f64)),
        "iconUrl" => text(&row.icon_url),
        "imageUrl" => text(&row.image_url),
        "actions" => text(&row.actions),
        "replyAction" => text(reply.map_or("", |a| a.id.as_str())),
        "replyLabel" => text(reply.map_or("", |a| a.title.as_str())),
        "silent" => QVariant::from(&n.silent),
        "ongoing" => QVariant::from(&n.ongoing),
        "replies" => text(&row.replies),
        "otpCode" => text(&row.otp_code),
        "live" => text(&row.live),
        "groupCount" => QVariant::from(&row.group_count),
        "groupIndex" => QVariant::from(&row.group_index),
        "groupExpanded" => QVariant::from(&row.group_expanded),
        "groupSummary" => text(&row.group_summary),
        "groupOrdinal" => QVariant::from(&row.group_ordinal),
        "showingAll" => QVariant::from(&row.showing_all),
        _ => QVariant::default(),
    }
}

fn rule_name(rule: AppRule) -> &'static str {
    match rule {
        AppRule::Show => "show",
        AppRule::Quiet => "quiet",
        AppRule::Hidden => "hidden",
    }
}

/// The apps list for Settings (see the `apps` property).
fn apps_json(s: &AppState) -> String {
    let mut apps: Vec<(&String, &String)> = s.app_names.iter().collect();
    apps.sort_by_key(|(app, name)| (name.to_lowercase(), (*app).clone()));
    serde_json::Value::from(
        apps.into_iter()
            .map(|(app, name)| {
                serde_json::json!({
                    "app": app,
                    "name": name,
                    "icon": s.app_icons.get(app).map(|p| icons::file_url(p)).unwrap_or_default(),
                    "rule": rule_name(s.app_rule(app)),
                })
            })
            .collect::<Vec<_>>(),
    )
    .to_string()
}

/// What the rows are made from, copied out of the shared state so they're
/// made after its lock is let go.
#[derive(Debug, Default)]
struct Snapshot {
    /// Notifications whose app isn't hidden, newest first.
    views: Vec<NotificationView>,
    device_names: HashMap<DeviceId, String>,
    icons: HashMap<String, PathBuf>,
    images: HashMap<(DeviceId, String), PathBuf>,
    replies: HashMap<(DeviceId, String), Vec<SentReply>>,
    /// Apps whose groups are expanded.
    expanded: HashSet<String>,
}

impl Snapshot {
    fn of(s: &AppState) -> Self {
        let mut snapshot = Self::default();
        let mut apps = HashSet::new();
        for view in &s.notifications {
            let n = &view.notification;
            if s.app_rule(&n.app) == AppRule::Hidden {
                continue;
            }
            if apps.insert(n.app.as_str()) {
                if let Some(icon) = s.app_icons.get(&n.app) {
                    snapshot.icons.insert(n.app.clone(), icon.clone());
                }
                if s.is_app_expanded(&n.app) {
                    snapshot.expanded.insert(n.app.clone());
                }
            }
            snapshot
                .device_names
                .entry(view.device)
                .or_insert_with(|| s.name_of(&view.device).unwrap_or_default());
            let key = (view.device, n.key.clone());
            if let Some(image) = s.notification_images.get(&key) {
                snapshot.images.insert(key.clone(), image.clone());
            }
            if let Some(replies) = s.replies.get(&key) {
                snapshot.replies.insert(key, replies.clone());
            }
            snapshot.views.push(view.clone());
        }
        snapshot
    }
}

fn make_row(snapshot: &Snapshot, view: &NotificationView, group: Group) -> Row {
    let n = &view.notification;
    let key = (view.device, n.key.clone());
    Row::new(
        view.clone(),
        snapshot.device_names.get(&view.device).cloned().unwrap_or_default(),
        snapshot.icons.get(&n.app).map(|p| icons::file_url(p)).unwrap_or_default(),
        snapshot.images.get(&key).map(|p| icons::file_url(p)).unwrap_or_default(),
        snapshot.replies.get(&key).map(Vec::as_slice).unwrap_or_default(),
        group,
    )
}

/// How many of an opened group's notifications show before "Show all".
const SHOWN: usize = 3;

/// Which rows the feed shows.
#[derive(Debug, Default)]
struct Filter {
    /// One phone's, or every phone's.
    device: Option<DeviceId>,
    category: Category,
    /// Opened groups showing all their notifications, by app.
    show_all: HashSet<String>,
    /// The phone's default SMS app (its texts are under Messages).
    sms_app: Option<String>,
}

/// `device`'s notifications (every phone's for `None`) in the order the feed
/// shows them: a Live Update alone, or an app's notifications newest first,
/// each where its newest notification is.
fn slots(snapshot: &Snapshot, device: Option<DeviceId>) -> Vec<Vec<&NotificationView>> {
    let mut slots: Vec<Vec<&NotificationView>> = Vec::new();
    let mut groups: HashMap<(DeviceId, &str), usize> = HashMap::new();
    for view in snapshot.views.iter().filter(|v| device.is_none_or(|d| v.device == d)) {
        if view.notification.live.is_some() {
            slots.push(vec![view]);
            continue;
        }
        match groups.entry((view.device, view.notification.app.as_str())) {
            Entry::Occupied(slot) => slots[*slot.get()].push(view),
            Entry::Vacant(slot) => {
                slot.insert(slots.len());
                slots.push(vec![view]);
            }
        }
    }
    slots
}

/// Builds the feed's rows: one per app group, under the filter, newest app
/// first. An opened group's latest few notifications follow it, then a More
/// row; a Live Update is a row of its own.
fn build_rows(snapshot: &Snapshot, filter: &Filter) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut ordinal = 0;
    for members in slots(snapshot, filter.device) {
        let Some((lead, older)) = members.split_first() else { continue };
        let app = lead.notification.app.as_str();
        let category = category_of(app, &members, filter.sms_app.as_deref());
        if filter.category != Category::All && filter.category != category {
            continue;
        }
        let count = i32::try_from(members.len()).unwrap_or(i32::MAX);
        if lead.notification.live.is_some() {
            let group = Group { kind: Kind::Item, category, count, ordinal, ..Group::default() };
            rows.push(make_row(snapshot, lead, group));
            ordinal += 1;
            continue;
        }
        let expanded = snapshot.expanded.contains(app);
        let showing_all = expanded && filter.show_all.contains(app);
        let group = |kind, index, summary| Group {
            kind,
            category,
            count,
            index,
            expanded,
            summary,
            ordinal,
            showing_all,
        };
        rows.push(make_row(snapshot, lead, group(Kind::Group, 0, group_summary(older))));
        if expanded {
            let shown = if showing_all { members.len() } else { SHOWN.min(members.len()) };
            for (index, member) in members.iter().take(shown).enumerate() {
                let index = i32::try_from(index).unwrap_or(i32::MAX - 1) + 1;
                rows.push(make_row(snapshot, member, group(Kind::Item, index, String::new())));
            }
            let shown = i32::try_from(shown).unwrap_or(i32::MAX);
            rows.push(make_row(snapshot, lead, group(Kind::More, shown, String::new())));
        }
        ordinal += 1;
    }
    rows
}

/// How many of `device`'s notifications are under each filter: all,
/// messages, calls and apps.
fn category_counts(snapshot: &Snapshot, device: Option<DeviceId>, sms_app: Option<&str>) -> [usize; 4] {
    let mut counts = [0; 4];
    for members in slots(snapshot, device) {
        let Some(lead) = members.first() else { continue };
        let at = match category_of(&lead.notification.app, &members, sms_app) {
            Category::Messages => 1,
            Category::Calls => 2,
            Category::All | Category::Apps => 3,
        };
        counts[0] += members.len();
        counts[at] += members.len();
    }
    counts
}

/// A collapsed group's older notifications, in brief.
fn group_summary(older: &[&NotificationView]) -> String {
    older
        .iter()
        .filter_map(|v| {
            v.notification
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .or_else(|| v.notification.text.as_deref().filter(|t| !t.trim().is_empty()))
        })
        .map(str::trim)
        .take(2)
        .collect::<Vec<_>>()
        .join(" · ")
}

fn key_of(row: &Row) -> (Kind, DeviceId, String) {
    let n = &row.view.notification;
    let id = if row.kind == Kind::Item { &n.key } else { &n.app };
    (row.kind, row.view.device, id.clone())
}

impl cxx_qt::Initialize for qobject::NotificationList {
    fn initialize(mut self: Pin<&mut Self>) {
        self.as_mut().on_device_changed(|list| list.refresh()).release();
        self.as_mut().on_category_changed(|list| list.refresh()).release();
        super::subscribe(
            self.qt_thread(),
            Changes::NOTIFICATIONS | Changes::DEVICES | Changes::APPS | Changes::HISTORY,
            Self::refresh,
        );
    }
}

impl qobject::NotificationList {
    fn refresh(mut self: Pin<&mut Self>) {
        let (snapshot, apps, history, undo_label) = core_host::host()
            .hub
            .read(|s| (Snapshot::of(s), apps_json(s), s.history_enabled, s.last_dismissed_label.clone()));
        let filter = Filter {
            device: String::from(&self.device).parse().ok(),
            category: Category::named(&String::from(&self.category)),
            show_all: self.show_all.clone(),
            sms_app: crate::messages::sms_app(),
        };
        let new = build_rows(&snapshot, &filter);
        let sms_app = filter.sms_app.as_deref();
        let [all, chats, calls, others] = category_counts(&snapshot, filter.device, sms_app);
        let counts = serde_json::json!({ "all": all, "messages": chats, "calls": calls, "apps": others });
        let counts = counts.to_string();
        if String::from(&self.category_counts) != counts {
            self.as_mut().set_category_counts(QString::from(&counts));
        }
        let groups = new.last().map_or(0, |r| r.group_ordinal + 1);
        if self.group_total != groups {
            self.as_mut().set_group_total(groups);
        }
        let mut per_device = HashMap::new();
        for view in &snapshot.views {
            *per_device.entry(view.device).or_insert(0) += 1;
        }
        self.as_mut().rust_mut().per_device = per_device;
        if String::from(&self.apps) != apps {
            self.as_mut().set_apps(QString::from(&apps));
        }
        if String::from(&self.undo_label) != undo_label {
            self.as_mut().set_undo_label(QString::from(&undo_label));
        }
        self.as_mut().set_history_enabled(history);
        let root = QModelIndex::default();
        for edit in diff(&self.rows, &new, key_of) {
            match edit {
                Edit::Remove(row) => {
                    let r = row as i32;
                    // SAFETY: row exists; begin/end are balanced.
                    unsafe {
                        self.as_mut().begin_remove_rows(&root, r, r);
                        self.as_mut().rust_mut().rows.remove(row);
                        self.as_mut().end_remove_rows();
                    }
                }
                Edit::Insert(row) => {
                    let r = row as i32;
                    // SAFETY: row <= len; begin/end are balanced.
                    unsafe {
                        self.as_mut().begin_insert_rows(&root, r, r);
                        self.as_mut().rust_mut().rows.insert(row, new[row].clone());
                        self.as_mut().end_insert_rows();
                    }
                }
                Edit::Change(row) => {
                    self.as_mut().rust_mut().rows[row] = new[row].clone();
                    let index = self.index(row as i32, 0, &root);
                    self.as_mut().data_changed(&index, &index, &QList::default());
                }
            }
        }
        self.set_count(i32::try_from(snapshot.views.len()).unwrap_or(i32::MAX));
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

    pub fn count_for(&self, device: &QString) -> i32 {
        let device = String::from(device).parse::<DeviceId>().ok();
        device.and_then(|d| self.per_device.get(&d).copied()).unwrap_or(0)
    }

    pub fn dismiss(&self, device: &QString, key: &QString) {
        let dev_str = String::from(device);
        let key_str = String::from(key);
        if let Ok(dev_id) = dev_str.parse::<DeviceId>() {
            let found = core_host::host().hub.read(|s| {
                s.notifications.iter().find(|n| n.device == dev_id && n.notification.key == key_str).cloned()
            });
            // The phone won't dismiss these, so it would only come back.
            if found.as_ref().is_some_and(|v| v.notification.ongoing) {
                return;
            }
            if let Some(view) = found {
                let label = if view.notification.app_name.is_empty() {
                    tr!("Notifications", "Notification dismissed")
                } else {
                    tr!("Notifications", "Dismissed from %1", view.notification.app_name)
                };
                core_host::host().hub.update(|s| s.record_dismissed(vec![view], label));
            }
        }
        crate::notifications::dismiss(&dev_str, key_str);
    }

    pub fn dismiss_group(&self, device: &QString, app: &QString) {
        let dev_str = String::from(device);
        let app_str = String::from(app);
        let Ok(dev_id) = dev_str.parse::<DeviceId>() else { return };
        let views: Vec<NotificationView> = core_host::host().hub.read(|s| {
            s.notifications
                .iter()
                .filter(|n| n.device == dev_id && n.notification.app == app_str && !n.notification.ongoing)
                .cloned()
                .collect()
        });
        if views.is_empty() {
            return;
        }
        let app_name = views[0].notification.app_name.clone();
        let count = views.len();
        let label = if count == 1 {
            tr!("Notifications", "Dismissed from %1", app_name)
        } else {
            tr!("Notifications", "Dismissed %1 from %2", count, app_name)
        };
        let keys: Vec<String> = views.iter().map(|v| v.notification.key.clone()).collect();
        core_host::host().hub.update(|s| s.record_dismissed(views, label));
        for key in keys {
            crate::notifications::dismiss(&dev_str, key);
        }
    }

    pub fn dismiss_all(&self, device: &QString) {
        let dev_str = String::from(device);
        let Ok(dev_id) = dev_str.parse::<DeviceId>() else { return };
        let views: Vec<NotificationView> = core_host::host().hub.read(|s| {
            s.notifications
                .iter()
                .filter(|n| {
                    n.device == dev_id
                        && !n.notification.ongoing
                        && s.app_rule(&n.notification.app) != AppRule::Hidden
                })
                .cloned()
                .collect()
        });
        if views.is_empty() {
            return;
        }
        let count = views.len();
        let label = if count == 1 {
            tr!("Notifications", "Notification dismissed")
        } else {
            tr!("Notifications", "Dismissed %1 notifications", count)
        };
        let keys: Vec<String> = views.iter().map(|v| v.notification.key.clone()).collect();
        core_host::host().hub.update(|s| s.record_dismissed(views, label));
        for key in keys {
            crate::notifications::dismiss(&dev_str, key);
        }
    }

    pub fn toggle_group(&self, app: &QString) {
        let app_str = String::from(app);
        core_host::host().hub.update(|s| s.toggle_app_expanded(&app_str));
    }

    pub fn show_all(mut self: Pin<&mut Self>, app: &QString, on: bool) {
        let app = String::from(app);
        let changed = if on {
            self.as_mut().rust_mut().show_all.insert(app)
        } else {
            self.as_mut().rust_mut().show_all.remove(&app)
        };
        if changed {
            self.refresh();
        }
    }

    pub fn undo_dismiss(&self) {
        core_host::host().hub.update(AppState::undo_dismiss);
    }

    pub fn clear_undo(&self) {
        core_host::host().hub.update(AppState::clear_undo);
    }

    pub fn run_action(&self, device: &QString, key: &QString, action: &QString) {
        crate::notifications::run_action(
            &String::from(device),
            String::from(key),
            String::from(action),
            None,
        );
    }

    pub fn app_rule(&self, app: &QString) -> QString {
        let rule = core_host::host().hub.read(|s| s.app_rule(&String::from(app)));
        QString::from(rule_name(rule))
    }

    pub fn set_app_rule(&self, app: &QString, rule: &QString) {
        let rule = match String::from(rule).as_str() {
            "quiet" => AppRule::Quiet,
            "hidden" => AppRule::Hidden,
            _ => AppRule::Show,
        };
        crate::notifications::set_app_rule(&String::from(app), rule);
    }

    pub fn set_history_enabled_by_user(&self, on: bool) {
        core_host::host().hub.update(|s| s.set_history_enabled(on));
    }

    pub fn clear_history(&self) {
        core_host::host().hub.update(AppState::clear_history);
    }

    pub fn copy_code(&self, code: &QString) {
        crate::notifications::copy_otp(&String::from(code));
    }

    fn set_history_enabled(mut self: Pin<&mut Self>, on: bool) {
        if self.history_enabled != on {
            self.as_mut().rust_mut().history_enabled = on;
            self.history_enabled_changed();
        }
    }

    fn set_apps(mut self: Pin<&mut Self>, apps: QString) {
        self.as_mut().rust_mut().apps = apps;
        self.apps_changed();
    }

    fn set_undo_label(mut self: Pin<&mut Self>, label: QString) {
        self.as_mut().rust_mut().undo_label = label;
        self.undo_label_changed();
    }

    pub fn reply(&self, device: &QString, key: &QString, action: &QString, text: &QString) {
        let text = String::from(text).trim().to_owned();
        if text.is_empty() {
            return;
        }
        crate::notifications::run_action(
            &String::from(device),
            String::from(key),
            String::from(action),
            Some(text),
        );
    }
}

#[cfg(test)]
mod tests {
    use g7link_core::{NodeEvent, Notification, NotificationAction};

    use super::*;

    #[test]
    fn roles_describe_the_notification() {
        let row = Row::new(
            NotificationView {
                device: DeviceId([7; 32]),
                notification: Notification {
                    key: "k".into(),
                    app: "com.chat".into(),
                    app_name: "Chat".into(),
                    title: Some("Sam".into()),
                    text: Some("Your verification code is 482913".into()),
                    sub: None,
                    when: 1_760_000_000_000,
                    actions: vec![
                        NotificationAction { id: "r".into(), title: "Reply".into(), reply: true },
                        NotificationAction { id: "m".into(), title: "Mute".into(), reply: false },
                    ],
                    silent: false,
                    ongoing: false,
                    icon: None,
                    image: None,
                    live: Some(g7link_core::NotificationLive {
                        v: 1,
                        progress: Some(40),
                        max: Some(100),
                        indeterminate: false,
                        chip: Some("5 min".into()),
                        segments: Vec::new(),
                        points: Vec::new(),
                        chronometer: false,
                        countdown: false,
                    }),
                    conversation: None,
                },
            },
            "Pixel".into(),
            String::new(),
            "file:///C:/data/cache/images/1.jpg".into(),
            &[SentReply { text: "On my way".into(), pending: true }],
            Group { count: 3, summary: "Alex · Taylor".into(), ..Group::default() },
        );
        let text = |role| role_value(&row, role).value::<QString>().map(String::from).unwrap_or_default();
        assert_eq!(text("title"), "Sam");
        assert_eq!(text("text"), "Your verification code is 482913");
        assert_eq!(text("otpCode"), "482913");
        assert_eq!(text("replyAction"), "r");
        assert_eq!(text("actions"), r#"[{"id":"m","title":"Mute"}]"#);
        assert_eq!(role_value(&row, "when").value::<f64>(), Some(1_760_000_000_000.0));
        assert_eq!(text("replies"), r#"[{"pending":true,"text":"On my way"}]"#);
        assert!(text("live").contains(r#""chip":"5 min""#));
        assert_eq!(role_value(&row, "groupCount").value::<i32>(), Some(3));
        assert_eq!(role_value(&row, "groupIndex").value::<i32>(), Some(0));
        assert_eq!(role_value(&row, "groupExpanded").value::<bool>(), Some(false));
        assert_eq!(text("groupSummary"), "Alex · Taylor");
    }

    fn note(key: &str, app: &str, title: &str, when: i64) -> Notification {
        Notification {
            key: key.into(),
            app: app.into(),
            app_name: app.into(),
            title: Some(title.into()),
            text: Some("body".into()),
            sub: None,
            when,
            actions: Vec::new(),
            silent: false,
            ongoing: false,
            icon: None,
            image: None,
            live: None,
            conversation: None,
        }
    }

    fn post(s: &mut AppState, device: DeviceId, notification: Notification) {
        s.apply(&NodeEvent::NotificationPosted { device, notification });
    }

    fn shape(rows: &[Row]) -> Vec<(&'static str, &str)> {
        rows.iter()
            .map(|r| {
                let n = &r.view.notification;
                (r.kind.name(), if r.kind == Kind::Item { n.key.as_str() } else { n.app.as_str() })
            })
            .collect()
    }

    #[test]
    fn one_row_per_app_newest_first_and_opening_shows_the_latest_few() {
        let phone = DeviceId([1; 32]);
        let mut s = AppState::default();
        for (i, key) in ["c1", "c2", "c3", "c4", "c5"].into_iter().enumerate() {
            post(&mut s, phone, note(key, "com.chat", key, 10 * i as i64));
        }
        post(&mut s, phone, note("m1", "com.mail", "Invoice", 25));
        let filter = Filter { device: Some(phone), ..Filter::default() };

        let rows = build_rows(&Snapshot::of(&s), &filter);
        assert_eq!(shape(&rows), [("group", "com.chat"), ("group", "com.mail")]);
        assert_eq!((rows[0].group_count, rows[0].view.notification.key.as_str()), (5, "c5"));
        assert_eq!(rows[0].group_summary, "c4 · c3");
        assert_eq!((rows[0].group_ordinal, rows[1].group_ordinal), (0, 1));

        // Opened: the latest three, then Show all / Clear.
        s.toggle_app_expanded("com.chat");
        let rows = build_rows(&Snapshot::of(&s), &filter);
        assert_eq!(
            shape(&rows),
            [
                ("group", "com.chat"),
                ("item", "c5"),
                ("item", "c4"),
                ("item", "c3"),
                ("more", "com.chat"),
                ("group", "com.mail")
            ]
        );
        assert_eq!(rows[1].group_index, 1);
        assert_eq!((rows[4].group_index, rows[4].group_count), (3, 5), "3 of 5 show");
        assert!(rows.iter().take(5).all(|r| r.group_ordinal == 0 && r.group_expanded));

        // Show all: every one of them.
        let all = Filter { show_all: HashSet::from(["com.chat".to_owned()]), ..filter };
        let rows = build_rows(&Snapshot::of(&s), &all);
        assert_eq!(rows.iter().filter(|r| r.kind == Kind::Item).count(), 5);
        assert!(rows[0].showing_all);
    }

    #[test]
    fn filters_sort_apps_into_messages_calls_and_apps() {
        let (phone, tablet) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let mut s = AppState::default();
        post(&mut s, phone, note("w1", "com.whatsapp", "Amina", 10));
        post(&mut s, phone, note("w2", "com.whatsapp", "Brian", 20));
        post(&mut s, phone, note("d1", "com.google.android.dialer", "Missed call", 30));
        post(&mut s, phone, note("g1", "com.google.android.gm", "Invoice", 40));
        // A reply box makes an unknown app a chat app.
        let mut chat = note("x1", "com.example.chat", "Sam", 50);
        chat.actions = vec![NotificationAction { id: "r".into(), title: "Reply".into(), reply: true }];
        post(&mut s, phone, chat);
        // The phone's own SMS app, by what it reported.
        post(&mut s, phone, note("t1", "com.example.sms", "Bank", 60));
        // Another phone's don't count here.
        post(&mut s, tablet, note("o1", "com.whatsapp", "Other", 70));

        let snapshot = Snapshot::of(&s);
        assert_eq!(category_counts(&snapshot, Some(phone), Some("com.example.sms")), [6, 4, 1, 1]);
        assert_eq!(category_counts(&snapshot, Some(phone), None), [6, 3, 1, 2], "unknown SMS app: an app");
        assert_eq!(category_counts(&snapshot, None, Some("com.example.sms"))[0], 7);

        let apps = |category| {
            let filter = Filter {
                device: Some(phone),
                category,
                sms_app: Some("com.example.sms".into()),
                ..Filter::default()
            };
            build_rows(&snapshot, &filter).iter().map(|r| r.view.notification.app.clone()).collect::<Vec<_>>()
        };
        assert_eq!(apps(Category::Messages), ["com.example.sms", "com.example.chat", "com.whatsapp"]);
        assert_eq!(apps(Category::Calls), ["com.google.android.dialer"]);
        assert_eq!(apps(Category::Apps), ["com.google.android.gm"]);
        assert_eq!(apps(Category::All).len(), 5);
        assert_eq!(Category::named("calls"), Category::Calls);
        assert_eq!(Category::named(""), Category::All);
    }

    #[test]
    fn a_live_update_is_a_row_of_its_own() {
        let phone = DeviceId([1; 32]);
        let mut s = AppState::default();
        post(&mut s, phone, note("c1", "com.chat", "Ann", 10));
        let mut ride = note("r1", "com.ride", "Ride", 50);
        ride.live = Some(g7link_core::NotificationLive {
            v: 1,
            progress: Some(1),
            max: Some(4),
            indeterminate: false,
            chip: None,
            segments: Vec::new(),
            points: Vec::new(),
            chronometer: false,
            countdown: false,
        });
        post(&mut s, phone, ride);
        let rows = build_rows(&Snapshot::of(&s), &Filter::default());
        assert_eq!(shape(&rows), [("item", "r1"), ("group", "com.chat")]);
        assert_eq!((rows[0].group_count, rows[0].group_index, rows[1].group_ordinal), (1, 0, 1));
    }
}
