// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQml
import QtQuick.Dialogs
import app.g7link

// Agents: Claude and Codex threads on this PC, and the sessions started here
// outside G7, on the left (what needs you, then a group per project with its
// threads and sessions, as the phone has them); on the right the open one
// (its transcript, what it asks, the reply box or Continue here, and its
// details), a new thread, or the setup. It doesn't need a paired phone.
Item {
    id: page
    property bool active: true
    property string deviceId
    property string deviceName
    // What the right side shows when nothing is open: "" (a new thread when
    // everything is set up, else the setup), "new" or "setup".
    property string pane: ""
    // The details beside the transcript, on a page too narrow to keep them.
    property bool detailsOpen: false
    // Date.now(), ticking every second while something runs.
    property real now: Date.now()
    // The session to continue once its folder is added ("" for none).
    property string continueAfterAdd: ""

    function parse(json, fallback) {
        try {
            const value = JSON.parse(json)
            return value === null || value === undefined ? fallback : value
        } catch (e) {
            return fallback
        }
    }
    readonly property var setup: parse(Agents.setup, ({}))
    readonly property var threads: parse(Agents.threads, [])
    readonly property var sessions: parse(Agents.sessions, [])
    readonly property var info: parse(Agents.openInfo, ({}))
    // The request the dock shows ({} for none): the oldest the typing guard
    // doesn't hold.
    readonly property var ask: parse(Agents.ask, ({}))
    readonly property bool asking: isThread && (ask.token || "").length > 0
    // It arrived while the user was typing: its buttons wait for a pause.
    readonly property bool askHeld: ask.held === true
    readonly property bool canAllow: asking && !askHeld && !Agents.busy
    function allowShown() {
        if (canAllow)
            Agents.answer(ask.token, "allow")
    }
    // Ctrl+Shift+X, far from Allow so a slip never lands on it.
    function denyShown() {
        if (canAllow)
            Agents.answer(ask.token, "deny")
    }
    readonly property var steps: parse(Agents.steps, [])
    readonly property var changes: parse(Agents.changes, [])
    // The question the dock shows rather than a request ({} for none).
    readonly property var question: parse(Agents.question, ({}))
    readonly property bool questioning: isThread && (question.token || "").length > 0
    // The open thread's composer around its text, its `@`/`/` list, and
    // text it must take.
    readonly property var composerState: parse(Agents.composer, ({}))
    readonly property var popup: parse(Agents.popup, ({}))
    readonly property var composerSet: parse(Agents.composerSet, ({}))
    readonly property var newPicker: parse(Agents.newPicker, ({}))
    // Phase 3: shelves, search, bringing in sessions, the diff, rewind.
    readonly property bool historyOn: setup.history === true
    readonly property bool undoOn: setup.undo === true
    readonly property var shelfRows: parse(Agents.shelf, null)
    readonly property var searchState: parse(Agents.search, ({}))
    readonly property var importState: parse(Agents.bringIn, ({}))
    readonly property var diffState: parse(Agents.diff, ({}))
    // Source control: the open thread's strip and panel, the Pull Requests
    // list, Setup's part and the new thread's worktree.
    readonly property var gitState: parse(Agents.git, ({}))
    readonly property var prsState: parse(Agents.prs, ({}))
    readonly property var gitSetup: parse(Agents.gitSetup, ({}))
    readonly property bool gitOn: isThread && gitState.available === true
    // The panel sits beside the transcript in a window of 1,000 px or more;
    // otherwise it lies over the thread.
    readonly property bool gitInline: Window.width >= 1000
    readonly property bool gitShown: gitOn && gitState.shown === true && !diffShown && !prDetailShown
    // Source control's slot beside the transcript: its panel, or the run panel.
    readonly property real gitWidth: (gitShown || runsShown) && gitInline ? 360 : 0
    function toggleGit() {
        if (page.gitOn)
            Agents.gitPanel(page.gitState.shown !== true)
    }
    // Project scripts (§X5.1): the Run menu in the header, the run panel in the same slot.
    readonly property var runsState: parse(Agents.runs, ({}))
    readonly property bool runsOn: isThread && runsState.available === true
    readonly property bool runsShown: runsOn && runsState.shown === true && !diffShown && !prDetailShown
    function toggleRuns() {
        if (page.runsOn)
            Agents.runPanel(page.runsState.shown !== true)
    }
    // "Edit scripts…": Setup, scrolled to the folder's card.
    function editScripts(folderId) {
        page.openSetup()
        Qt.callLater(() => {
            const y = setupArea.folderY(folderId)
            if (y >= 0)
                setupView.contentY = Math.max(0, Math.min(setupArea.y + y - 8, setupView.contentHeight - setupView.height))
        })
    }
    // A pull request, read-only (§X6.6): over the thread it was opened from,
    // or over the Pull Requests list.
    readonly property var prDetailState: parse(Agents.prDetail, ({}))
    readonly property bool prDetailShown: prDetailState.open === true && !diffShown
    // "Ask in a new thread…": the new-thread form carries this instead of a prompt.
    property var reviewChip: ({})
    readonly property bool reviewing: (reviewChip.number || 0) > 0
    function askInNewThread(repo, number, folderId) {
        Agents.prDetailClose()
        Agents.prsClose()
        page.reviewChip = { repo: repo, number: number, folderId: folderId }
        page.openNew()
        if (folderId.length > 0 && page.usableFolders.some((f) => f.id === folderId))
            newView.folder = folderId
    }
    function openPrs() {
        Agents.close()
        Agents.importClose()
        page.pane = "prs"
        Agents.prsOpen("mine", "")
    }
    readonly property var rewindState: parse(Agents.rewind, ({}))
    readonly property var focusState: parse(Agents.focus, ({}))
    // "all", "archived" or "snoozed".
    property string shelf: "all"
    // Threads picked with Ctrl+click, for the bulk bar.
    property var picked: []
    // The thread whose title is being renamed in place ("" for none).
    property string renamingId: ""
    // The thread the row menu is about.
    property var menuRow: ({})
    // The message a search hit opened, lit up for a moment.
    property string highlightItem: ""
    readonly property bool isPreview: openKind === "preview"
    readonly property bool diffShown: diffState.turn !== undefined && (isThread || diffState.scope === "pr")
    readonly property bool readOnly: isThread && (info.readonly || "").length > 0
    // Searching messages: the host's search replaces the sections.
    readonly property string searchText: search.text
    readonly property bool searching: historyOn && searchText.trim().length > 0
    // A usage window near its end, or reached: one amber line over the composer.
    readonly property var limit: composerState.limit || ({})
    readonly property var context: info.context || ({})
    readonly property var mcp: info.mcp || ({})
    // A usage window's name as the CLI gives it: minutes or a word.
    function windowName(w) {
        switch (w) {
        case "300":
        case "five_hour": return qsTr("5-hour")
        case "10080":
        case "seven_day": return qsTr("weekly")
        default: return w
        }
    }
    // This PC will say when the reached limit resets (its alarm is on): a bell.
    readonly property bool limitTold: composerState.limitTold === true && limit.state === "reached"
    readonly property string limitText: {
        if (!limit.window)
            return ""
        const resets = limit.resetsMs > 0 ? clock(limit.resetsMs) : ""
        if (limitTold && resets.length > 0)
            return qsTr("Paused until %1 · you'll be told").arg(dayClock(limit.resetsMs))
        if (limit.state === "reached")
            return resets.length > 0 ? qsTr("Usage limit reached · resets %1").arg(resets) : qsTr("Usage limit reached")
        const name = windowName(limit.window)
        return resets.length > 0 ? qsTr("%1 limit almost used · resets %2").arg(name).arg(resets)
                                 : qsTr("%1 limit almost used").arg(name)
    }
    // The thread's model and effort, as the details say them.
    readonly property string modelLine: {
        const label = (composerState.picker || {}).label
        if (!label || label.model.length === 0)
            return qsTr("Default model")
        return label.effort.length > 0 ? label.model + " · " + label.effort : label.model
    }
    readonly property bool connected: Agents.status === "connected"
    readonly property var readyAgents: connected
        ? (setup.agents || []).filter((a) => a.installed && a.signedIn && a.problem.length === 0) : []
    readonly property var usableFolders: connected ? (setup.folders || []).filter((f) => f.exists) : []
    readonly property bool ready: readyAgents.length > 0 && usableFolders.length > 0
    readonly property string openKind: Agents.openKind
    readonly property bool isThread: openKind === "thread"
    readonly property bool isSession: openKind === "session"
    readonly property string shown: openKind.length > 0 ? openKind
        : pane === "import" ? "import"
        : pane === "prs" ? "prs"
        : (pane === "setup" || (pane !== "new" && !ready) ? "setup" : "new")
    readonly property bool narrow: width < 720
    // On a narrow page the list fills it until something is picked.
    readonly property bool listOnly: narrow && openKind.length === 0 && pane.length === 0
    // The details sit beside the transcript only while it keeps 480 px
    // (from about 1230 px of window); otherwise they fold into Details.
    // The thread list beside the rest, unless the user hid it (Ctrl+[). A
    // narrow page shows the list or the thread, never both.
    readonly property bool listShown: !narrow && !Agents.listHidden
    // Its width, moving as it opens and closes.
    property real listWidth: listShown ? 300 : 0
    Behavior on listWidth {
        enabled: !Theme.reduceMotion
        NumberAnimation {
            duration: page.listShown ? Theme.largeIn : Theme.largeOut
            easing.type: Easing.BezierSpline
            easing.bezierCurve: page.listShown ? Theme.easeEnter : Theme.easeExit
        }
    }
    // The details fit beside the transcript while it keeps 480 px.
    readonly property bool inspectorFits: width - (listShown ? 300 : 0) - 250 >= 480
    // They sit beside it when they fit, unless the user hid them (Ctrl+]);
    // otherwise they fold into Details, over the transcript.
    readonly property bool inspectorInline: inspectorFits && !Agents.inspectorHidden
    property real inspectorWidth: (isThread || isSession) && inspectorInline && !gitShown && !runsShown ? 250 : 0
    Behavior on inspectorWidth {
        enabled: !Theme.reduceMotion
        NumberAnimation {
            duration: page.inspectorInline ? Theme.largeIn : Theme.largeOut
            easing.type: Easing.BezierSpline
            easing.bezierCurve: page.inspectorInline ? Theme.easeEnter : Theme.easeExit
        }
    }
    readonly property bool detailsShown: (isThread || isSession) && (inspectorInline || (detailsOpen && !inspectorFits))
    function toggleList() {
        if (narrow)
            return
        // The list's search field doesn't keep the keyboard once it's hidden.
        if (listShown && search.input.activeFocus)
            page.forceActiveFocus()
        Agents.hideList(!Agents.listHidden)
    }
    function toggleDetails() {
        if (inspectorFits)
            Agents.hideInspector(!Agents.inspectorHidden)
        else
            detailsOpen = !detailsOpen
    }
    // Tabler's plug (MIT, as Icons.qml credits).
    readonly property string plugIcon: "M9.8 6l8.2 8.2-2 2.1a5.8 5.8 0 1 1-8.3-8.3zM4 20l3.5-3.5M15 4l-3.5 3.5M20 9l-3.5 3.5"
    // Tabler's layout-sidebar (MIT, as Icons.qml credits).
    readonly property string listIcon: "M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9 4v16"
    // Tabler's layout-sidebar-right (MIT, as Icons.qml credits): the details panel's toggle.
    readonly property string detailsIcon: "M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM15 4v16"
    // Lucide's columns-2, list-checks and server (ISC, as AgentGitGlyphs.qml
    // credits): the phone beside the PC, its Plan pane, and a dev server.
    readonly property string besideIcon: "M4 3h16a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM12 3v18"
    readonly property string listChecksIcon: "M3 17l2 2 4-4M3 7l2 2 4-4M13 6h8M13 12h8M13 18h8"
    readonly property string serverIcon: "M5 2h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2zM5 14h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2zM6 6h.01M6 18h.01"

    readonly property string phase: info.state || ""
    readonly property bool running: isThread && (phase === "working" || phase === "waiting")
    readonly property string agentName: info.agent_name || ""
    readonly property real since: (info.since_ms || 0) > 0 ? info.since_ms : (info.updated_ms || 0)
    readonly property var runningStep: {
        for (let i = steps.length - 1; i >= 0; --i) {
            if (steps[i].status === "running")
                return steps[i]
        }
        return null
    }
    readonly property bool anyRunning: threads.some((t) => t.state === "working" || t.state === "waiting")
        || sessions.some((s) => s.state === "working" || s.state === "needs_you")

    // ---- The list, with the search applied ----
    readonly property string query: search.text.trim().toLocaleLowerCase()
    function matches(row) {
        return query.length === 0
            || (row.title + " " + row.folder + " " + row.agent_name).toLocaleLowerCase().indexOf(query) >= 0
    }
    // As the phone has it (AgentProjects): what needs you, then a group per
    // folder holding its threads and the sessions started in it, the busiest first.
    readonly property var listedThreads: threads.filter((t) => matches(t))
    readonly property var listedSessions: sessions.filter((s) => matches(s))
    readonly property var needsItems: AgentProjects.needsYou(listedThreads, listedSessions)
    readonly property var groups: AgentProjects.groups(listedThreads, listedSessions)
    readonly property string newestFolder: AgentProjects.newest(groups)
    // Folders the user opened or closed ([[folder, open], ...]) and those
    // showing every row; a filter opens them all.
    property var folderChoices: []
    property var foldersShowingAll: []
    readonly property bool filtering: query.length > 0
    function folderOpen(group) {
        return filtering || AgentProjects.isOpen(group, newestFolder, folderChoices)
    }
    function toggleFolder(group) {
        page.folderChoices = AgentProjects.chose(page.folderChoices, group.folder, !folderOpen(group))
    }
    function folderRows(group) {
        return folderOpen(group) ? AgentProjects.shown(group, filtering || foldersShowingAll.indexOf(group.folder) >= 0) : []
    }
    function showAllOf(group) {
        if (page.foldersShowingAll.indexOf(group.folder) < 0)
            page.foldersShowingAll = page.foldersShowingAll.concat([group.folder])
    }
    readonly property var shelfShown: (shelfRows || []).filter((t) => matches(t))
    function showShelf(value) {
        page.shelf = value
        page.picked = []
        Agents.showShelf(value === "all" ? "main" : value)
    }
    // The list's rows in the order it shows them, for Alt+Up and Alt+Down; a
    // row that needs you is also in its folder, and counts once.
    readonly property var shownRows: {
        const thread = (t) => ({ kind: "thread", id: t.id })
        if (searching)
            return (searchState.groups || []).map((g) => ({ kind: "thread", id: g.thread }))
        if (shelf !== "all")
            return shelfShown.map(thread)
        const seen = new Set()
        const rows = []
        for (const i of needsItems.concat(...groups.map((g) => folderRows(g)))) {
            const key = i.kind + ":" + i.id
            if (!seen.has(key)) {
                seen.add(key)
                rows.push({ kind: i.kind, id: i.id })
            }
        }
        return rows
    }
    // Opens the row `by` away from the open one, stopping at the ends.
    function stepRow(by) {
        const rows = page.shownRows
        if (rows.length === 0)
            return
        const at = rows.findIndex((r) => r.kind === page.openKind && r.id === Agents.openId)
        const next = at < 0 ? (by > 0 ? 0 : rows.length - 1) : at + by
        if (next >= 0 && next < rows.length && next !== at)
            page.openRow(rows[next].kind, rows[next].id)
    }
    readonly property string setupSummary: {
        const folders = (setup.folders || []).length
        let phones = 0
        AppController.togglesRevision
        for (let i = 0; i < DeviceList.count; ++i) {
            const t = AppController.deviceToggles(DeviceList.deviceIdAt(i)).find((x) => x.name === "agents")
            if (t && t.on)
                phones++
        }
        const f = qsTr("%n folder(s)", "", folders)
        return phones > 0 ? f + " · " + qsTr("%n phone(s)", "", phones) : f
    }

    // ---- Words and times ----
    function ago(ms) {
        const minutes = Math.max(0, Math.floor((page.now - ms) / 60000))
        if (minutes < 1)
            return qsTr("now")
        if (minutes < 60)
            return qsTr("%1 min").arg(minutes)
        const hours = Math.floor(minutes / 60)
        return hours < 24 ? qsTr("%1 h").arg(hours) : qsTr("%1 d").arg(Math.floor(hours / 24))
    }
    function span(ms) {
        const minutes = Math.max(1, Math.floor((page.now - ms) / 60000))
        if (minutes < 60)
            return qsTr("%1 min").arg(minutes)
        const hours = Math.floor(minutes / 60)
        return hours < 24 ? qsTr("%1 h").arg(hours) : qsTr("%1 d").arg(Math.floor(hours / 24))
    }
    function elapsed(ms) {
        const total = Math.max(0, Math.floor((page.now - ms) / 1000))
        const h = Math.floor(total / 3600)
        const m = Math.floor((total % 3600) / 60)
        const s = String(total % 60).padStart(2, "0")
        return h > 0 ? h + ":" + String(m).padStart(2, "0") + ":" + s : m + ":" + s
    }
    function clock(ms) {
        return ms > 0 ? new Date(ms).toLocaleTimeString(Qt.locale(), Locale.ShortFormat) : ""
    }
    // "18:00" today, else the day and the time.
    function dayClock(ms) {
        const when = new Date(ms)
        return new Date(page.now).toDateString() === when.toDateString()
            ? clock(ms) : when.toLocaleDateString(Qt.locale(), "ddd") + " " + clock(ms)
    }
    readonly property string stateWord: {
        if (isSession) {
            switch (phase) {
            case "needs_you": return info.guess ? qsTr("May need you") : qsTr("Needs you")
            case "working": return qsTr("Working")
            case "idle": return qsTr("Idle")
            // It stopped at its agent's usage limit.
            case "limited":
                return (info.resets_ms || 0) > 0 ? qsTr("Limit reached · resets %1").arg(dayClock(info.resets_ms))
                                                 : qsTr("Limit reached")
            default: return qsTr("Ended")
            }
        }
        switch (phase) {
        case "waiting": return qsTr("Needs you")
        case "working": return qsTr("Working")
        case "done": return qsTr("Done")
        case "failed": return qsTr("Failed")
        case "interrupted": return qsTr("Stopped")
        default: return ""
        }
    }
    // Where it was started, for the header.
    readonly property string originText: {
        if (isSession) {
            if (info.source === "terminal")
                return qsTr("started in a terminal")
            if (info.source === "desktop_app")
                return info.agent === "codex" ? qsTr("started in the Codex app") : qsTr("started in the Claude app")
            return qsTr("started in another app")
        }
        if (info.from === "phone")
            return qsTr("started from %1").arg(info.device)
        if (info.from === "cli")
            return qsTr("started in a terminal")
        return info.continued ? qsTr("continued from a session") : ""
    }
    // A phone shows this thread now, so this PC doesn't notify about it.
    readonly property string openOnText: (info.open_on || "").length > 0 ? qsTr("open on %1").arg(info.open_on) : ""
    // The phone that shows this thread's Changes, Plan or Requests beside
    // the PC (§V9.1): { device, name, pane }.
    readonly property var beside: isThread ? (info.beside || ({})) : ({})
    readonly property bool besideShown: (beside.name || "").length > 0 && (beside.device || "").length > 0
    readonly property var besidePanes: ["changes", "plan", "requests"]
    function paneName(word) {
        return word === "plan" ? qsTr("Plan") : word === "requests" ? qsTr("Requests") : qsTr("Changes")
    }
    function paneIcon(word) {
        return word === "plan" ? page.listChecksIcon : word === "requests" ? Icons.bell : Icons.edit
    }
    // A dev server the thread started (§V9.2): { port, lan, phones: [{ id, name }] }.
    readonly property var server: isThread ? (info.server || ({})) : ({})
    readonly property int serverPort: server.port || 0
    // Where it was started, for the details.
    readonly property string originLine: {
        if (isSession) {
            if (info.source === "terminal")
                return qsTr("Started in a terminal")
            if (info.source === "desktop_app")
                return info.agent === "codex" ? qsTr("Started in the Codex app") : qsTr("Started in the Claude app")
            return qsTr("Started in another app")
        }
        if (info.from === "phone")
            return qsTr("Started from %1").arg(info.device)
        if (info.from === "cli")
            return qsTr("Started in a terminal")
        return info.continued ? qsTr("Continued from a session") : qsTr("Started on this PC")
    }
    // Where a session runs, as whole sentences.
    readonly property string followingText: {
        if (info.source === "terminal")
            return qsTr("You're following this session. It runs in a terminal.")
        if (info.source === "desktop_app")
            return info.agent === "codex" ? qsTr("You're following this session. It runs in the Codex app.")
                                          : qsTr("You're following this session. It runs in the Claude app.")
        return qsTr("You're following this session. It runs in another app.")
    }
    readonly property string bannerText: {
        if (info.source === "terminal")
            return qsTr("Started in a terminal on this PC. You can read it here.")
        if (info.source === "desktop_app")
            return info.agent === "codex" ? qsTr("Started in the Codex app on this PC. You can read it here.")
                                          : qsTr("Started in the Claude app on this PC. You can read it here.")
        return qsTr("Started in another app on this PC. You can read it here.")
    }
    readonly property string stillOpenText: {
        if (info.source === "terminal")
            return qsTr("Still open in a terminal. Close it there to continue here.")
        if (info.source === "desktop_app")
            return info.agent === "codex" ? qsTr("Still open in the Codex app. Close it there to continue here.")
                                          : qsTr("Still open in the Claude app. Close it there to continue here.")
        return qsTr("Still open in another app. Close it there to continue here.")
    }
    readonly property string waitingText: {
        const exact = !info.guess
        if (info.source === "terminal")
            return exact ? qsTr("%1 is waiting for you in the terminal.").arg(agentName)
                         : qsTr("%1 may be waiting in the terminal. Check it on %2.").arg(agentName).arg(AppController.deviceName)
        if (info.source === "desktop_app")
            return exact ? qsTr("%1 is waiting for you in the %1 app.").arg(agentName)
                         : qsTr("%1 may be waiting in the %1 app. Check it on %2.").arg(agentName).arg(AppController.deviceName)
        return exact ? qsTr("%1 is waiting for you in another app.").arg(agentName)
                     : qsTr("%1 may be waiting in another app. Check it on %2.").arg(agentName).arg(AppController.deviceName)
    }
    readonly property string clockText: {
        switch (phase) {
        case "working": return qsTr("Working for %1").arg(span(since))
        case "waiting":
        case "needs_you": return qsTr("Needs you for %1").arg(span(since))
        case "idle": return qsTr("Idle for %1").arg(span(since))
        case "ended": return qsTr("Ended %1").arg(clock(info.updated_ms || 0))
        default: return qsTr("Last active %1").arg(clock(info.updated_ms || 0))
        }
    }

    // ---- What the page does ----
    function startNew() {
        page.reviewChip = ({})
        openNew()
    }
    function openNew() {
        Agents.close()
        page.pane = "new"
        Qt.callLater(() => page.reviewing ? newView.forceActiveFocus() : taskInput.forceActiveFocus())
    }
    function openSetup() {
        Agents.close()
        Agents.importClose()
        page.pane = "setup"
        Agents.storeInfo()
    }
    function openImport() {
        Agents.close()
        page.pane = "import"
        Agents.importOpen()
    }
    // A search hit: its thread, at the message (seq "0": the newest).
    function openHit(thread, seq) {
        page.pane = ""
        page.detailsOpen = false
        Agents.importClose()
        Agents.openHit(thread, seq)
    }
    function markThreads(ids, what) {
        if (ids.length > 0)
            Agents.mark(JSON.stringify(ids), what, 0)
    }
    function togglePick(id) {
        const at = page.picked.indexOf(id)
        page.picked = at >= 0 ? page.picked.filter((x) => x !== id) : page.picked.concat([id]).slice(0, 50)
    }
    function openRowMenu(row, x, y) {
        page.menuRow = row
        const p = page.mapFromItem(null, x, y)
        rowMenu.openAt(p.x, p.y, false, true)
    }
    // The snooze times for `ids`, at (x, y) on the page.
    function snooze(ids, x, y, agentName, resets) {
        snoozeMenu.ids = ids
        snoozeMenu.agentName = agentName
        snoozeMenu.limitResets = resets
        snoozeMenu.show(x, y, y > page.height / 2)
    }
    // Lights up the message a hit opened at, once its row is in the timeline.
    property int focusTries: 0
    function applyFocus() {
        const item = page.focusState.item || ""
        if (item.length === 0)
            return
        const row = AgentTimeline.rowOfItem(item)
        if (row < 0) {
            if (page.focusTries++ < 20)
                focusRetry.restart()
            return
        }
        transcript.follow = false
        transcript.positionViewAtIndex(row, ListView.Center)
        page.highlightItem = item
        highlightTimer.restart()
    }
    onFocusStateChanged: {
        page.focusTries = 0
        applyFocus()
    }
    Timer {
        id: focusRetry
        interval: 100
        onTriggered: page.applyFocus()
    }
    Timer {
        id: highlightTimer
        interval: 1200
        onTriggered: page.highlightItem = ""
    }
    // Ctrl+Shift+D (Ctrl+D deletes a character in text fields on Linux): the last turn's changes, or back from the diff.
    readonly property int lastChangedTurn: isThread ? (composerState.lastChanged || 0) : 0
    function toggleDiff() {
        if (page.diffShown)
            page.back()
        else if (page.lastChangedTurn > 0)
            Agents.diffOpen(page.lastChangedTurn, "")
    }
    function openRow(kind, id) {
        page.pane = ""
        page.detailsOpen = false
        if (kind === "session")
            Agents.openSession(id)
        else
            Agents.openThread(id)
    }
    function back() {
        if (page.diffShown) {
            Agents.diffClose()
            return
        }
        if (page.prDetailShown) {
            Agents.prDetailClose()
            return
        }
        if (page.isPreview) {
            Agents.previewClose()
            return
        }
        Agents.close()
        if (page.pane === "import")
            Agents.importClose()
        if (page.pane === "prs")
            Agents.prsClose()
        page.pane = ""
    }
    // "Bring in 1" from a preview: back to the list, where it shows how it goes.
    function bringInPreview() {
        const id = Agents.openId
        Agents.previewClose()
        Agents.importStart(id)
    }
    function addFolder(sessionId) {
        page.continueAfterAdd = sessionId
        folderPicker.open()
    }

    // While the window is active the host hears which thread is on screen,
    // and skips its "needs you" and "finished" notifications.
    readonly property bool windowActive: Window.active
    onActiveChanged: Agents.pageVisible(active)
    // Setup's History line: what the host keeps, read when it shows.
    onShownChanged: {
        if (shown === "setup") {
            Agents.storeInfo()
            Agents.gitSetupRead(false)
        }
        // The review chip belongs to the form it was put in.
        if (shown !== "new")
            reviewChip = ({})
    }
    onWindowActiveChanged: Agents.windowActive(windowActive)
    // Code, tool targets, outputs and diff words, coloured by the agent
    // palette (plans/agent-colour.md §2).
    readonly property string codeColors: AgentPalette.bridgeJson
    onCodeColorsChanged: Agents.setCodeColors(codeColors)
    Component.onCompleted: {
        Agents.setCodeColors(codeColors)
        Agents.windowActive(windowActive)
        Agents.pageVisible(active)
    }
    Component.onDestruction: Agents.pageVisible(false)

    Timer {
        interval: page.anyRunning || page.running || (page.runsState.running || 0) > 0 ? 1000 : 30000
        repeat: true
        running: page.active
        onTriggered: page.now = Date.now()
    }

    // ---- Footer and Actions (Ctrl B) ----
    readonly property string footerLabel: shown === "setup" ? qsTr("Agents · Setup")
        : Agents.needsYou === 1 ? qsTr("Agents · 1 needs you")
        : Agents.needsYou > 1 ? qsTr("Agents · %1 need you").arg(Agents.needsYou) : qsTr("Agents")
    readonly property var primaryAction: {
        if (shown === "setup")
            return { label: qsTr("Add folder…"), keys: "Ctrl O", run: () => folderPicker.open() }
        if (diffShown)
            return { label: qsTr("Next file"), keys: "Ctrl J", run: () => Agents.diffStep(1) }
        if (isPreview)
            return { label: qsTr("Bring in %n", "", 1), keys: "", run: () => page.bringInPreview() }
        if (shown === "import" && (importState.selected || 0) > 0)
            return { label: qsTr("Bring in %n", "", importState.selected), keys: "", run: () => Agents.importStart("") }
        // With words in the composer, Ctrl+Enter never allows.
        if (canAllow && composer.sayWhy && composer.hasText)
            return { label: qsTr("Deny and send"), keys: "Enter", run: () => composer.submit(false) }
        if (canAllow && !composer.hasText)
            return { label: qsTr("Allow"), keys: "Ctrl Enter", run: () => page.allowShown() }
        // While a question waits only the dock answers it.
        if (questioning) {
            const last = (question.ask || {}).last === true
            return { label: last ? qsTr("Send answers") : qsTr("Next"), keys: "",
                     run: () => last ? Agents.questionSend() : Agents.questionStep(true) }
        }
        if (isThread && (!running || composerState.control === true))
            return { label: qsTr("Send"), keys: "Enter", run: () => composer.submit(false) }
        if (isSession && info.can_continue === true)
            return { label: qsTr("Continue here"), keys: "Ctrl Enter", run: () => continueSheet.open() }
        return { label: qsTr("New thread"), keys: "Ctrl N", run: () => page.startNew() }
    }
    readonly property var screenActions: [
        { label: qsTr("New thread"), icon: Icons.plus, tint: Theme.accent, keys: "Ctrl N", run: () => page.startNew() },
        { label: qsTr("Search threads"), icon: Icons.search, keys: "Ctrl F", run: () => search.input.forceActiveFocus() },
        { label: qsTr("Stop"), icon: Icons.stopSquare, tint: Theme.tintRed, keys: "Ctrl .", enabled: page.running,
          run: () => Agents.stop() },
        { label: qsTr("Deny"), icon: Icons.close, keys: "Ctrl Shift X", enabled: page.canAllow && !composer.hasText,
          run: () => page.denyShown() },
        { label: qsTr("Next thread"), icon: Icons.chevronDown, keys: "Alt ↓", enabled: page.shownRows.length > 0,
          run: () => page.stepRow(1) },
        { label: qsTr("Previous thread"), icon: Icons.chevronUp, keys: "Alt ↑", enabled: page.shownRows.length > 0,
          run: () => page.stepRow(-1) },
        { label: qsTr("Model and effort"), icon: Icons.settings, keys: "Ctrl M", enabled: page.isThread && composer.settingsOpen,
          run: () => composer.openPicker() },
        { label: qsTr("Mode"), icon: Icons.shield, keys: "Ctrl Shift M", enabled: page.isThread && composer.settingsOpen,
          run: () => composer.openModeMenu() },
        { label: qsTr("Show changes"), icon: Icons.edit, keys: "Ctrl D", checked: page.diffShown,
          enabled: page.diffShown || page.lastChangedTurn > 0, run: () => page.toggleDiff() },
        { label: qsTr("Run panel"), icon: Icons.play, keys: "Ctrl Shift R", checked: page.runsShown,
          enabled: page.runsOn, run: () => page.toggleRuns() },
        { label: qsTr("Continue here"), icon: Icons.send, tint: Theme.accent, keys: "Ctrl Enter",
          enabled: page.isSession && page.info.can_continue === true, run: () => continueSheet.open() },
        { label: qsTr("Open in a terminal"), icon: Icons.terminal, enabled: page.isThread && !page.running,
          run: () => Agents.openInTerminal() },
        { label: qsTr("Copy as Markdown"), icon: Icons.copy, enabled: page.isThread || page.isSession,
          run: () => Agents.copyMarkdown() },
        { label: qsTr("Export…"), icon: Icons.download, enabled: page.isThread || page.isSession,
          run: () => Agents.exportMarkdown() },
        { label: qsTr("Load earlier"), icon: Icons.history, enabled: Agents.more && !Agents.loading,
          run: () => Agents.loadEarlier() },
        { label: page.listShown ? qsTr("Hide the thread list") : qsTr("Show the thread list"), icon: page.listIcon,
          keys: "Ctrl [", checked: page.listShown, enabled: !page.narrow, run: () => page.toggleList() },
        { label: qsTr("Details"), icon: page.detailsIcon, keys: "Ctrl ]", checked: page.detailsShown,
          enabled: page.isThread || page.isSession, run: () => page.toggleDetails() },
        { label: qsTr("Delete thread"), icon: Icons.trash, tint: Theme.tintRed, enabled: page.isThread && !page.running,
          run: () => deleteSheet.ask([Agents.openId]) },
        { label: qsTr("Bring in earlier sessions"), icon: Icons.download, checked: page.shown === "import",
          enabled: page.historyOn, run: () => page.openImport() },
        { label: qsTr("Rename"), icon: Icons.edit, keys: "F2", enabled: page.historyOn && page.isThread,
          run: () => page.renamingId = Agents.openId },
        { label: qsTr("Setup"), icon: Icons.settings, checked: page.shown === "setup", run: () => page.openSetup() },
        { label: qsTr("Add folder…"), icon: Icons.folder, tint: Theme.tintAmber, run: () => folderPicker.open() },
        { label: qsTr("Refresh"), icon: Icons.refresh, run: () => Agents.refresh() },
        { label: qsTr("Close"), icon: Icons.close, keys: "Esc", enabled: page.openKind.length > 0 || page.pane.length > 0,
          run: () => page.back() }
    ]

    Shortcut {
        sequences: [StandardKey.New]
        enabled: page.active
        onActivated: page.startNew()
    }
    // The source-control panel.
    Shortcut {
        sequence: "Ctrl+Shift+G"
        enabled: page.active && page.gitOn
        onActivated: page.toggleGit()
    }
    Shortcut {
        sequence: "Ctrl+["
        enabled: page.active && !page.narrow
        onActivated: page.toggleList()
    }
    Shortcut {
        sequence: "Ctrl+]"
        enabled: page.active && (page.isThread || page.isSession)
        onActivated: page.toggleDetails()
    }
    Shortcut {
        sequences: [StandardKey.Find]
        enabled: page.active
        onActivated: search.input.forceActiveFocus()
    }
    Shortcut {
        sequences: [StandardKey.Open]
        enabled: page.active && page.shown === "setup" && page.connected
        onActivated: folderPicker.open()
    }
    Shortcut {
        sequence: "Ctrl+Return"
        enabled: page.active && page.isSession && page.info.can_continue === true && !continueSheet.opened
        onActivated: continueSheet.open()
    }
    // Allow the shown request: only while the composer is empty, so a habit
    // of Ctrl+Enter to send can never allow a command.
    Shortcut {
        sequences: ["Ctrl+Return", "Ctrl+Enter"]
        enabled: page.active && page.canAllow && !composer.hasText && !linkSheet.opened && !dockMenu.opened
                 && !attachMenu.opened && !headMenu.opened && !AppController.commandPaletteOpen
        onActivated: page.allowShown()
    }
    // 1–9 pick a question's answers while the keyboard isn't in a text field.
    Instantiator {
        model: 9
        delegate: Shortcut {
            required property int index
            sequence: String(index + 1)
            enabled: page.active && page.questioning && page.question.held !== true && !composer.inputFocused
                     && !linkSheet.opened && !dockMenu.opened && !attachMenu.opened && !headMenu.opened
                     && !AppController.commandPaletteOpen
            onActivated: Agents.questionPick(index)
        }
    }
    Shortcut {
        sequence: "Esc"
        enabled: page.active && (page.openKind.length > 0 || page.pane.length > 0)
                 && !continueSheet.opened && !folderSheet.opened && !deleteSheet.opened && !restartSheet.opened
                 && !linkSheet.opened && !dockMenu.opened && !planMenu.opened && !threadPicker.opened
                 && !newFormPicker.opened && !composer.popupOpen && !AppController.commandPaletteOpen
                 && !rewindDialog.opened && !rowMenu.opened && !snoozeMenu.opened && !bulkMenu.opened
                 && !attachMenu.opened && !headMenu.opened && page.renamingId.length === 0
        onActivated: page.back()
    }
    // Phase 6's keys (§X8), each only while no menu, sheet or palette is open.
    readonly property bool keysFree: !AppController.commandPaletteOpen && !composer.popupOpen
        && !continueSheet.opened && !folderSheet.opened && !deleteSheet.opened && !restartSheet.opened
        && !linkSheet.opened && !dockMenu.opened && !attachMenu.opened && !headMenu.opened && !besideMenu.opened
        && !rowMenu.opened && !snoozeMenu.opened && !bulkMenu.opened && !diffMenu.opened && !planMenu.opened
        && !gitMenu.opened && !modeMenu.opened && !runMenu.opened && !rewindDialog.opened && !branchMenu.opened
        && !switchDialog.opened && !worktreeDialog.opened && !mergeDialog.opened && !prDialog.opened
        && !threadPicker.opened && !newFormPicker.opened && page.renamingId.length === 0
    // Deny the shown request: only while the composer is empty (with words
    // in it, Enter denies and says why).
    Shortcut {
        sequence: "Ctrl+Shift+X"
        enabled: page.active && page.canAllow && !composer.hasText && page.keysFree
        onActivated: page.denyShown()
    }
    Shortcut {
        sequence: "Ctrl+."
        enabled: page.active && page.running && page.keysFree
        onActivated: Agents.stop()
    }
    Shortcut {
        sequence: "Alt+Down"
        enabled: page.active && page.keysFree
        onActivated: page.stepRow(1)
    }
    Shortcut {
        sequence: "Alt+Up"
        enabled: page.active && page.keysFree
        onActivated: page.stepRow(-1)
    }
    Shortcut {
        sequence: "Ctrl+M"
        enabled: page.active && page.isThread && composer.settingsOpen && page.keysFree
        onActivated: composer.openPicker()
    }
    Shortcut {
        sequence: "Ctrl+Shift+M"
        enabled: page.active && page.isThread && composer.settingsOpen && page.keysFree
        onActivated: composer.openModeMenu()
    }
    Shortcut {
        sequence: "Ctrl+Shift+D"
        enabled: page.active && page.keysFree && (page.diffShown || page.lastChangedTurn > 0)
        onActivated: page.toggleDiff()
    }
    // The run panel (§X5.1).
    Shortcut {
        sequence: "Ctrl+Shift+R"
        enabled: page.active && page.runsOn && page.keysFree
        onActivated: page.toggleRuns()
    }
    // The diff's files, one after another.
    Shortcut {
        sequence: "Ctrl+J"
        enabled: page.active && page.diffShown
        onActivated: Agents.diffStep(1)
    }
    Shortcut {
        sequence: "Ctrl+K"
        enabled: page.active && page.diffShown
        onActivated: Agents.diffStep(-1)
    }
    Shortcut {
        sequence: "F2"
        enabled: page.active && page.historyOn && page.isThread && page.renamingId.length === 0
                 && !composer.inputFocused
        onActivated: page.renamingId = Agents.openId
    }

    FileDialog {
        id: imagePicker
        title: qsTr("Add image")
        fileMode: FileDialog.OpenFiles
        nameFilters: [qsTr("Images (*.png *.jpg *.jpeg *.webp *.gif)")]
        onAccepted: Agents.addImages(JSON.stringify(Array.from(selectedFiles, (u) => String(u))))
    }

    FolderDialog {
        id: folderPicker
        title: qsTr("Add a folder for agents")
        onAccepted: folderSheet.ask(Preferences.localFolder(selectedFolder.toString()))
    }

    // Export…: opens once the Markdown is here, on the name the host suggests.
    FileDialog {
        id: exportDialog
        title: qsTr("Export as Markdown")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "md"
        nameFilters: [qsTr("Markdown (*.md)"), qsTr("All files (*)")]
        onAccepted: Agents.saveExport(selectedFile.toString())
        onRejected: Agents.dropExport()
    }
    Connections {
        target: Agents
        function onExportFileChanged() {
            const file = Agents.exportFile
            if (file.length === 0 || exportDialog.visible)
                return
            exportDialog.currentFolder = file.slice(0, file.lastIndexOf("/"))
            exportDialog.selectedFile = file
            exportDialog.open()
        }
    }

    // A list section: its title and count (or neither, under a folder's
    // header), then its rows: threads and sessions as AgentProjects lists
    // them ({ kind, id, row }).
    component ListSection: Column {
        id: section
        property string title
        property var items: []
        property bool titled: true
        property bool counted: true
        property real now: Date.now()
        // Marks offered (the host keeps them), threads picked, the one being renamed.
        property bool marks: false
        property var pickedIds: []
        property string renamingId: ""
        signal picked(string kind, string id)
        signal pickToggled(string id)
        signal markRequested(string id, string what)
        signal menuRequested(var row, real x, real y)
        signal renameRequested(string id)
        signal renamed(string id, string title)
        signal renameCancelled
        visible: items.length > 0
        topPadding: titled ? 6 : 0
        Item {
            visible: section.titled
            width: section.width
            height: visible ? 28 : 0
            Txt {
                x: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                text: section.title
                role: "label"
                muted: true
            }
            Txt {
                visible: section.counted
                anchors.right: parent.right
                anchors.rightMargin: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                text: String(section.items.length)
                role: "bodySmall"
                muted: true
            }
        }
        Repeater {
            model: section.items
            delegate: AgentRow {
                id: listRow
                required property var modelData
                readonly property bool isThread: modelData.kind === "thread"
                width: section.width
                item: modelData.row
                session: !isThread
                now: section.now
                marks: section.marks
                picking: isThread && section.pickedIds.length > 0
                picked: isThread && section.pickedIds.indexOf(modelData.id) >= 0
                renaming: isThread && section.renamingId === modelData.id
                selected: Agents.openKind === modelData.kind && Agents.openId === modelData.id
                onClicked: section.picked(listRow.modelData.kind, listRow.modelData.id)
                onPickToggled: section.pickToggled(listRow.modelData.id)
                onPinRequested: section.markRequested(listRow.modelData.id, listRow.modelData.row.pinned ? "unpin" : "pin")
                onArchiveRequested: section.markRequested(listRow.modelData.id,
                                                          listRow.modelData.row.archived ? "unarchive" : "archive")
                onMenuRequested: (x, y) => section.menuRequested(listRow.modelData.row, x, y)
                onRenameRequested: section.renameRequested(listRow.modelData.id)
                onRenamed: (title) => section.renamed(listRow.modelData.id, title)
                onRenameCancelled: section.renameCancelled()
            }
        }
    }

    // A folder in the list (AgentProjects' group): its glyph and name, then
    // what's in it. Open, its counts and branch; closed, what needs you or
    // runs in it and when it last moved. A click opens or closes it.
    component FolderHeader: Item {
        id: head
        property var group: ({})
        property bool open: false
        property bool interactive: true
        property real now: Date.now()
        signal toggled
        readonly property string name: (group.folder || "").length > 0 ? group.folder : qsTr("No folder")
        readonly property string counts: {
            const threads = group.threads || 0
            const sessions = group.sessions || 0
            const parts = []
            if (threads > 0)
                parts.push(threads === 1 ? qsTr("1 thread") : qsTr("%1 threads").arg(threads))
            if (sessions > 0)
                parts.push(sessions === 1 ? qsTr("1 session") : qsTr("%1 sessions").arg(sessions))
            return parts.join(" · ")
        }
        readonly property string needsText: (group.needsYou || 0) === 0 ? ""
            : group.needsYou === 1 ? qsTr("1 needs you") : qsTr("%1 need you").arg(group.needsYou)
        readonly property string runningText: (group.running || 0) === 0 ? ""
            : group.running === 1 ? qsTr("1 running") : qsTr("%1 running").arg(group.running)
        readonly property string branch: group.branch || ""
        // "2 min", "1 h", "3 d".
        function ago(ms) {
            const minutes = Math.max(0, Math.floor((head.now - ms) / 60000))
            if (minutes < 1)
                return qsTr("now")
            if (minutes < 60)
                return qsTr("%1 min").arg(minutes)
            const hours = Math.floor(minutes / 60)
            return hours < 24 ? qsTr("%1 h").arg(hours) : qsTr("%1 d").arg(Math.floor(hours / 24))
        }
        readonly property string summary: open
            ? [counts, branch].filter((s) => s.length > 0).join(" · ")
            : [needsText, runningText, needsText.length + runningText.length > 0 ? "" : counts, ago(group.lastMs || 0)]
                .filter((s) => s.length > 0).join(" · ")

        implicitHeight: headTexts.implicitHeight + 16
        activeFocusOnTab: interactive
        Accessible.role: Accessible.Button
        Accessible.name: head.name
        Accessible.description: head.summary
        Accessible.onPressAction: if (head.interactive) head.toggled()
        Keys.onPressed: (event) => {
            if (head.interactive && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space)) {
                head.toggled()
                event.accepted = true
            }
        }

        StateFill {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: Theme.raised
            on: head.interactive && headMouse.containsMouse
        }
        Rectangle {
            anchors.fill: parent
            radius: Theme.rowRadius
            color: "transparent"
            visible: Theme.focusVisible(head)
            border.width: 2
            border.color: Theme.focusRing
        }
        MouseArea {
            id: headMouse
            anchors.fill: parent
            enabled: head.interactive
            hoverEnabled: true
            cursorShape: head.interactive ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: head.toggled()
        }
        Icon {
            id: folderGlyph
            x: Theme.rowPadding - 2
            y: 8 + Math.round((nameText.lineHeight - height) / 2)
            width: 16
            height: 16
            path: Icons.folder
            color: Theme.tintAmber
        }
        Column {
            id: headTexts
            anchors.left: folderGlyph.right
            anchors.leftMargin: 8
            anchors.right: chevron.left
            anchors.rightMargin: 8
            y: 8
            spacing: 1
            Txt {
                id: nameText
                width: parent.width
                text: head.name
                weight: 600
                maximumLineCount: 1
            }
            // Closed, what needs you and what runs come first, in their colours.
            Row {
                id: summaryLine
                width: parent.width
                readonly property bool needsShown: !head.open && head.needsText.length > 0
                readonly property bool runningShown: !head.open && head.runningText.length > 0
                Txt {
                    visible: summaryLine.needsShown
                    text: head.needsText
                    role: "bodySmall"
                    color: AgentPalette.needsYou
                }
                Txt {
                    visible: summaryLine.runningShown
                    text: (summaryLine.needsShown ? " · " : "") + head.runningText
                    role: "bodySmall"
                    color: AgentPalette.working
                }
                Txt {
                    readonly property string rest: head.open ? head.summary
                        : (summaryLine.needsShown || summaryLine.runningShown ? "" : head.counts + " · ") + head.ago(head.group.lastMs || 0)
                    width: Math.max(0, summaryLine.width - x)
                    text: (summaryLine.needsShown || summaryLine.runningShown ? " · " : "") + rest
                    role: "bodySmall"
                    muted: true
                    maximumLineCount: 1
                }
            }
        }
        Icon {
            id: chevron
            anchors.right: parent.right
            anchors.rightMargin: Theme.rowPadding
            y: 8 + Math.round((nameText.lineHeight - height) / 2)
            width: 14
            height: 14
            path: head.open ? Icons.chevronUp : Icons.chevronDown
            color: Theme.surfaceContentVariant
        }
    }


    // ---- The list ----
    Item {
        id: listPane
        // Its content keeps its width while the pane opens and closes.
        readonly property real contentWidth: page.narrow ? width : 300
        visible: page.narrow ? page.listOnly : page.listWidth > 0
        width: page.narrow ? page.width : page.listWidth
        height: page.height
        clip: true

        Column {
            id: listTop
            x: 8
            y: 8
            width: listPane.contentWidth - 16
            spacing: 8
            Rectangle {
                width: parent.width
                height: Theme.rowHeight
                radius: Theme.rowRadius
                color: Theme.raised
                ActionRow {
                    anchors.fill: parent
                    iconPath: Icons.plus
                    tint: Theme.accent
                    title: qsTr("New thread")
                    keys: "Ctrl N"
                    selected: page.shown === "new" && !page.listOnly
                    onClicked: page.startNew()
                }
            }
            // The Pull Requests list, read through the GitHub CLI.
            ActionRow {
                visible: page.gitSetup.available === true
                width: parent.width
                height: visible ? Theme.rowHeight : 0
                iconPath: AgentGitGlyphs.pullRequest
                title: qsTr("Pull requests")
                selected: page.shown === "prs" && !page.listOnly
                onClicked: page.openPrs()
            }
            Field {
                id: search
                width: parent.width
                placeholder: qsTr("Search threads")
                iconPath: Icons.search
                Keys.onEscapePressed: (event) => {
                    if (search.text.length > 0) {
                        search.text = ""
                        event.accepted = true
                    }
                }
                onTextChanged: if (page.historyOn) Agents.searchThreads(search.text, page.searchState.archived === true)
                // Enter opens the first thread found, at its first message.
                onAccepted: {
                    const group = (page.searchState.groups || [])[0]
                    if (page.searching && group)
                        page.openHit(group.thread, ((group.hits || [])[0] || {}).seq || "0")
                }
            }
            // The agent host is still the version from before an update, kept
            // because threads run on it.
            Rectangle {
                visible: Agents.outdated
                width: parent.width
                height: visible ? updateColumn.height + 20 : 0
                radius: Theme.radiusMd
                color: Theme.surfaceContainerLow
                border.width: 1
                border.color: Theme.tintAmber
                Column {
                    id: updateColumn
                    x: 10
                    y: 10
                    width: parent.width - 20
                    spacing: 8
                    Txt {
                        width: parent.width
                        text: qsTr("Restart agents to finish the update")
                        weight: 500
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                    Txt {
                        width: parent.width
                        text: page.anyRunning
                            ? qsTr("Threads are still running on the previous version. It updates by itself once they finish.")
                            : qsTr("The previous version is still running.")
                        role: "bodySmall"
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                    Button {
                        variant: "tonal"
                        size: "sm"
                        text: qsTr("Restart agents")
                        busy: Agents.status === "connecting"
                        onClicked: restartSheet.open()
                    }
                }
            }
        }

        Flickable {
            id: listFlick
            x: 8
            width: listPane.contentWidth - 16
            anchors.top: listTop.bottom
            anchors.topMargin: 4
            anchors.bottom: searchFoot.top
            anchors.bottomMargin: 4
            contentHeight: listColumn.height
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            Column {
                id: listColumn
                width: listFlick.width
                bottomPadding: 8
                // Search: the messages that match, grouped by thread.
                AgentSearch {
                    visible: page.searching
                    width: parent.width
                    search: page.searchState
                    now: page.now
                    openId: page.isThread ? Agents.openId : ""
                    onOpen: (thread, seq) => page.openHit(thread, seq)
                    onArchivedToggled: (on) => Agents.searchThreads(page.searchText, on)
                }
                Column {
                    visible: !page.searching && page.shelf === "all"
                    width: parent.width
                    ListSection {
                        width: parent.width
                        title: qsTr("Needs you")
                        items: page.needsItems
                        now: page.now
                        marks: page.historyOn
                        pickedIds: page.picked
                        renamingId: page.renamingId
                        onPicked: (kind, id) => kind === "thread" && page.picked.length > 0 ? page.togglePick(id) : page.openRow(kind, id)
                        onPickToggled: (id) => page.togglePick(id)
                        onMarkRequested: (id, what) => page.markThreads([id], what)
                        onMenuRequested: (row, x, y) => page.openRowMenu(row, x, y)
                        onRenameRequested: (id) => page.renamingId = id
                        onRenamed: (id, title) => {
                            Agents.rename(id, title)
                            page.renamingId = ""
                        }
                        onRenameCancelled: page.renamingId = ""
                    }
                    Item {
                        visible: page.groups.length > 0
                        width: parent.width
                        height: visible ? 34 : 0
                        Txt {
                            x: Theme.rowPadding
                            anchors.bottom: parent.bottom
                            anchors.bottomMargin: 6
                            text: qsTr("Projects")
                            role: "label"
                            muted: true
                        }
                    }
                    // A folder: its header, then its rows (those that need
                    // you or run, then three more) and "n more".
                    Repeater {
                        model: page.groups
                        delegate: Item {
                            id: folderBlock
                            required property var modelData
                            readonly property bool open: page.folderOpen(modelData)
                            readonly property var rows: page.folderRows(modelData)
                            readonly property int hidden: open ? modelData.items.length - rows.length : 0
                            width: listColumn.width
                            height: folderColumn.height + 6
                            // One card per folder, as the phone has them: an outline,
                            // so its rows keep their hover and selection fills.
                            Rectangle {
                                width: parent.width
                                height: folderColumn.height
                                radius: Theme.radiusMd
                                color: "transparent"
                                border.width: 1
                                border.color: Theme.outlineVariant
                            }
                            Column {
                                id: folderColumn
                                width: parent.width
                                FolderHeader {
                                    width: parent.width
                                    group: folderBlock.modelData
                                    open: folderBlock.open
                                    interactive: !page.filtering
                                    now: page.now
                                    onToggled: page.toggleFolder(folderBlock.modelData)
                                }
                                ListSection {
                                    width: parent.width
                                    titled: false
                                    items: folderBlock.rows
                                    now: page.now
                                    marks: page.historyOn
                                    pickedIds: page.picked
                                    renamingId: page.renamingId
                                    onPicked: (kind, id) => kind === "thread" && page.picked.length > 0 ? page.togglePick(id) : page.openRow(kind, id)
                                    onPickToggled: (id) => page.togglePick(id)
                                    onMarkRequested: (id, what) => page.markThreads([id], what)
                                    onMenuRequested: (row, x, y) => page.openRowMenu(row, x, y)
                                    onRenameRequested: (id) => page.renamingId = id
                                    onRenamed: (id, title) => {
                                        Agents.rename(id, title)
                                        page.renamingId = ""
                                    }
                                    onRenameCancelled: page.renamingId = ""
                                }
                                ActionRow {
                                    visible: folderBlock.hidden > 0
                                    width: parent.width
                                    height: visible ? Theme.rowHeight : 0
                                    iconPath: Icons.chevronDown
                                    title: qsTr("%1 more").arg(folderBlock.hidden)
                                    titleColor: Theme.surfaceContentVariant
                                    onClicked: page.showAllOf(folderBlock.modelData)
                                }
                            }
                        }
                    }
                    // The shelves, under the folders.
                    Column {
                        visible: page.historyOn
                        width: parent.width
                        topPadding: 8
                        ActionRow {
                            width: parent.width
                            iconPath: Icons.tray
                            title: qsTr("Archived")
                            onClicked: page.showShelf("archived")
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                                path: Icons.chevronRight
                                color: Theme.surfaceContentVariant
                            }
                        }
                        ActionRow {
                            width: parent.width
                            iconPath: Icons.moon
                            title: qsTr("Snoozed")
                            onClicked: page.showShelf("snoozed")
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                                path: Icons.chevronRight
                                color: Theme.surfaceContentVariant
                            }
                        }
                    }
                }
                // The Archived or Snoozed shelf, and the way back.
                Column {
                    visible: !page.searching && page.shelf !== "all"
                    width: parent.width
                    ActionRow {
                        width: parent.width
                        iconPath: Icons.back
                        title: qsTr("Back to projects")
                        onClicked: page.showShelf("all")
                    }
                    ListSection {
                        width: parent.width
                        title: page.shelf === "archived" ? qsTr("Archived") : qsTr("Snoozed")
                        items: AgentProjects.items(page.shelfShown, [])
                        now: page.now
                        marks: page.historyOn
                        pickedIds: page.picked
                        renamingId: page.renamingId
                        onPicked: (kind, id) => kind === "thread" && page.picked.length > 0 ? page.togglePick(id) : page.openRow(kind, id)
                        onPickToggled: (id) => page.togglePick(id)
                        onMarkRequested: (id, what) => page.markThreads([id], what)
                        onMenuRequested: (row, x, y) => page.openRowMenu(row, x, y)
                        onRenameRequested: (id) => page.renamingId = id
                        onRenamed: (id, title) => {
                            Agents.rename(id, title)
                            page.renamingId = ""
                        }
                        onRenameCancelled: page.renamingId = ""
                    }

                    Txt {
                        visible: page.shelfRows !== null && page.shelfShown.length === 0 && page.query.length === 0
                        x: Theme.rowPadding
                        topPadding: 12
                        text: page.shelf === "archived" ? qsTr("Nothing archived.") : qsTr("Nothing snoozed.")
                        role: "bodySmall"
                        muted: true
                    }
                }
                Txt {
                    visible: Agents.sessionsUnsupported && page.connected
                    x: Theme.rowPadding
                    width: parent.width - 2 * Theme.rowPadding
                    topPadding: 12
                    text: qsTr("Restart agents to see the sessions started on this PC.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
                Txt {
                    visible: page.threads.length === 0 && page.sessions.length === 0 && page.connected
                             && page.shelf === "all" && !page.searching
                    x: Theme.rowPadding
                    width: parent.width - 2 * Theme.rowPadding
                    topPadding: 12
                    text: qsTr("No threads yet. Start one with New thread.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
                TextAction {
                    visible: page.threads.length === 0 && page.connected && page.historyOn
                             && page.shelf === "all" && !page.searching
                    x: Theme.rowPadding
                    text: qsTr("Bring in earlier sessions")
                    iconPath: Icons.download
                    onClicked: page.openImport()
                }
                Txt {
                    visible: page.query.length > 0 && !page.searching && page.shelf === "all"
                             && page.groups.length === 0
                             && page.threads.length + page.sessions.length > 0
                    x: Theme.rowPadding
                    width: parent.width - 2 * Theme.rowPadding
                    topPadding: 12
                    text: qsTr("Nothing matches.")
                    role: "bodySmall"
                    muted: true
                }
            }
            TouchpadGlide { view: listFlick }
        }
        G7ScrollBar {
            flickable: listFlick
            anchors.top: listFlick.top
            anchors.bottom: listFlick.bottom
            anchors.right: listFlick.right
            anchors.rightMargin: -6
        }

        // What search covers.
        Txt {
            id: searchFoot
            visible: page.searching
            x: 8 + Theme.rowPadding
            width: listPane.contentWidth - 16 - 2 * Theme.rowPadding
            height: visible ? implicitHeight : 0
            anchors.bottom: setupRow.top
            anchors.bottomMargin: visible ? 8 : 0
            text: qsTr("Searches threads in G7. Bring in earlier sessions to search them.")
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }

        ActionRow {
            id: setupRow
            visible: page.picked.length === 0
            x: 8
            width: listPane.contentWidth - 16
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 8
            iconPath: Icons.settings
            tint: Theme.surfaceContentVariant
            title: qsTr("Setup")
            meta: page.connected ? page.setupSummary : ""
            selected: page.shown === "setup" && !page.listOnly
            onClicked: page.openSetup()
        }

        // Several threads picked: what can be done to them all.
        Rectangle {
            id: bulkBar
            readonly property var rows: page.threads.concat(page.shelfRows || [])
                .filter((t) => page.picked.indexOf(t.id) >= 0)
            readonly property bool allPinned: rows.length > 0 && rows.every((t) => t.pinned === true)
            readonly property bool allArchived: rows.length > 0 && rows.every((t) => t.archived === true)
            readonly property bool allSnoozed: rows.length > 0 && rows.every((t) => (t.snoozedUntil || 0) > 0)
            visible: page.picked.length > 0
            x: 8
            width: listPane.contentWidth - 16
            height: setupRow.height
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 8
            radius: Theme.rowRadius
            color: Theme.selection
            Txt {
                x: 10
                anchors.left: parent.left
                anchors.right: bulkButtons.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("%n selected", "", page.picked.length)
                role: "bodySmall"
                weight: 600
                maximumLineCount: 1
            }
            Row {
                id: bulkButtons
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                IconButton {
                    iconPath: Icons.pin
                    label: bulkBar.allPinned ? qsTr("Unpin") : qsTr("Pin")
                    onClicked: page.markThreads(page.picked, bulkBar.allPinned ? "unpin" : "pin")
                }
                IconButton {
                    iconPath: Icons.tray
                    label: bulkBar.allArchived ? qsTr("Unarchive") : qsTr("Archive")
                    onClicked: {
                        page.markThreads(page.picked, bulkBar.allArchived ? "unarchive" : "archive")
                        page.picked = []
                    }
                }
                IconButton {
                    id: bulkSnooze
                    iconPath: Icons.moon
                    label: qsTr("Snooze")
                    onClicked: {
                        const p = page.mapFromItem(bulkSnooze, 0, 0)
                        page.snooze(page.picked, p.x, p.y - 8, "", 0)
                    }
                }
                IconButton {
                    iconPath: Icons.trash
                    label: qsTr("Delete")
                    onClicked: deleteSheet.ask(page.picked)
                }
                IconButton {
                    id: bulkMore
                    iconPath: Icons.more
                    label: qsTr("More")
                    onClicked: {
                        const p = page.mapFromItem(bulkMore, 0, 0)
                        bulkMenu.openAt(p.x, p.y, true, false)
                    }
                }
                IconButton {
                    iconPath: Icons.close
                    label: qsTr("Clear")
                    onClicked: page.picked = []
                }
            }
        }

        Rectangle {
            visible: !page.narrow
            x: page.listWidth - 1
            width: 1
            height: parent.height
            color: Theme.line
        }
    }

    // ---- The right side ----
    Item {
        id: mainPane
        visible: !page.listOnly
        x: page.narrow ? 0 : page.listWidth
        width: page.width - x
        height: page.height
        clip: true

        // Header: the title and where it stands, then Details and Stop.
        Item {
            id: head
            width: parent.width
            height: 56
            IconButton {
                id: backButton
                visible: page.narrow || page.diffShown || page.prDetailShown || page.isPreview || page.shown === "import"
                         || page.shown === "prs"
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                iconPath: Icons.back
                label: page.diffShown && page.diffState.scope === "pr" ? qsTr("Back to the pull request")
                    : (page.diffShown || page.prDetailShown) && page.isThread ? qsTr("Back to the thread")
                    : qsTr("Back to the list")
                onClicked: page.back()
            }
            // Hides or shows the thread list (Ctrl+[).
            IconButton {
                id: listToggle
                visible: !page.narrow && !backButton.visible
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                iconPath: page.listIcon
                checked: page.listShown
                label: page.listShown ? qsTr("Hide the thread list") : qsTr("Show the thread list")
                onClicked: page.toggleList()
            }
            Column {
                anchors.left: backButton.visible ? backButton.right : listToggle.right
                anchors.leftMargin: 8
                anchors.right: headButtons.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                spacing: 1
                Txt {
                    width: parent.width
                    text: page.diffShown ? qsTr("Changes")
                        : page.prDetailShown ? qsTr("Pull request #%1").arg(page.prDetailState.number || 0)
                        : page.shown === "setup" ? qsTr("Setup")
                        : page.shown === "import" ? qsTr("Bring in earlier sessions")
                        : page.shown === "prs" ? qsTr("Pull requests")
                        : page.shown === "new" ? qsTr("New thread") : (page.info.title || "")
                    role: "title"
                    maximumLineCount: 1
                }
                Txt {
                    visible: page.diffShown || page.prDetailShown || page.isPreview || page.shown === "import"
                    width: parent.width
                    text: page.diffShown && page.diffState.scope === "pr" ? ((page.prDetailState.pr || {}).title || "")
                        : page.prDetailShown ? [page.prDetailState.repo || "", (page.prDetailState.pr || {}).title || ""]
                                               .filter((x) => x.length > 0).join(" · ")
                        : page.diffShown ? (page.info.title || "")
                        : page.isPreview ? [page.info.agent_name, page.info.shown_cwd,
                                            new Date(page.info.updated_ms || 0).toLocaleDateString(Qt.locale(), Locale.ShortFormat)]
                                           .filter((x) => (x || "").length > 0).join(" · ")
                        : (page.importState.total || 0) > 0
                            ? qsTr("%n sessions on this PC", "", page.importState.total) : ""
                    role: "bodySmall"
                    muted: true
                    maximumLineCount: 1
                }
                Row {
                    width: parent.width
                    spacing: 6
                    visible: (page.isThread || page.isSession) && !page.diffShown && !page.prDetailShown
                    AgentStateMark {
                        anchors.verticalCenter: parent.verticalCenter
                        phase: page.phase
                        guess: page.isSession && page.info.guess === true && page.phase === "needs_you"
                    }
                    Txt {
                        id: stateLine
                        width: Math.min(implicitWidth, parent.width - 18 - (forkLink.visible ? forkLink.width + 6 : 0))
                        anchors.verticalCenter: parent.verticalCenter
                        text: [page.stateWord, page.agentName, page.isSession ? page.info.shown_cwd
                               : (page.info.worktree || "").length > 0 ? page.info.folder + " · " + qsTr("worktree") : page.info.folder,
                               page.info.found === true ? qsTr("Brought in") : page.originText, page.openOnText]
                            .filter((s) => (s || "").length > 0).join(" · ")
                        role: "bodySmall"
                        muted: true
                        maximumLineCount: 1
                    }
                    // A fork links back to the thread it came from.
                    Txt {
                        id: forkLink
                        readonly property var from: page.info.forkOf || ({})
                        visible: page.isThread && (from.title || "").length > 0
                        width: Math.min(implicitWidth, parent.width / 2)
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Fork of %1").arg(from.title || "")
                        role: "bodySmall"
                        color: Theme.accent
                        maximumLineCount: 1
                        Accessible.role: Accessible.Link
                        Accessible.name: text
                        Accessible.onPressAction: page.openRow("thread", forkLink.from.id)
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: page.openRow("thread", forkLink.from.id)
                        }
                    }
                }
                Txt {
                    visible: page.shown === "setup" || page.shown === "new"
                    width: parent.width
                    text: page.shown === "new"
                        ? qsTr("It runs on this PC, in one of your agent folders.")
                        : (page.narrow ? qsTr("Agents, folders and phones")
                                       : qsTr("Agents, the folders they may work in, and your phones"))
                    role: "bodySmall"
                    muted: true
                    maximumLineCount: 1
                }
            }
            Row {
                id: headButtons
                anchors.right: parent.right
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                // The diff's scope: this turn, or the whole thread.
                Segmented {
                    visible: page.diffShown && (page.diffState.canWhole === true || page.diffState.scope === "uncommitted"
                                                || page.diffState.scope === "pr")
                    options: page.diffState.scope === "pr" ? [{ value: "pr", label: qsTr("Pull request") }]
                        : (page.gitOn ? [{ value: "uncommitted", label: qsTr("Uncommitted") }] : [])
                        .concat(page.diffState.canTurn === true ? [{ value: "turn", label: qsTr("This turn") }] : [])
                        .concat(page.diffState.canWhole === true ? [{ value: "thread", label: qsTr("Whole thread") }] : [])
                    value: page.diffState.scope || "turn"
                    onPicked: (value) => Agents.diffScope(value)
                }
                AgentGitStrip {
                    id: headStrip
                    visible: page.gitOn && !page.diffShown && !page.prDetailShown && headStrip.repo
                    anchors.verticalCenter: parent.verticalCenter
                    info: page.gitState.strip || ({})
                    open: page.gitShown
                    onClicked: page.toggleGit()
                }
                // The Run menu: the folder's scripts (§X5.1); a ring while one runs.
                Item {
                    visible: page.runsOn && !page.diffShown && !page.prDetailShown
                    anchors.verticalCenter: parent.verticalCenter
                    width: runButton.width
                    height: runButton.height
                    IconButton {
                        id: runButton
                        iconPath: Icons.play
                        checked: runMenu.opened || page.runsShown
                        label: qsTr("Run")
                        onClicked: {
                            const p = page.mapFromItem(runButton, runButton.width, runButton.height)
                            runMenu.openAt(p.x, p.y)
                        }
                    }
                    Spinner {
                        visible: (page.runsState.running || 0) > 0
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        width: 10
                        height: 10
                    }
                }
                // The phone beside this PC and the pane it shows: the menu switches it (§V9.1).
                Rectangle {
                    id: besidePill
                    function openMenu() {
                        const p = page.mapFromItem(besidePill, besidePill.width, besidePill.height)
                        besideMenu.openAt(p.x, p.y, false, true)
                    }
                    visible: page.besideShown && !page.diffShown && !page.prDetailShown
                    anchors.verticalCenter: parent.verticalCenter
                    width: besideRow.implicitWidth + 20
                    height: 28
                    radius: Theme.radiusSm
                    color: besideMenu.opened ? Theme.activeFill : besideHover.hovered ? Theme.selection : Theme.raised
                    border.width: besidePill.activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                    activeFocusOnTab: true
                    Accessible.role: Accessible.ButtonMenu
                    Accessible.name: besideText.text
                    Accessible.onPressAction: besidePill.openMenu()
                    Keys.onPressed: (event) => {
                        if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                            besidePill.openMenu()
                            event.accepted = true
                        }
                    }
                    Row {
                        id: besideRow
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 14
                            height: 14
                            path: page.besideIcon
                            color: Theme.surfaceContentVariant
                        }
                        Txt {
                            id: besideText
                            anchors.verticalCenter: parent.verticalCenter
                            width: Math.min(implicitWidth, page.narrow ? 140 : 240)
                            text: qsTr("%1 shows %2").arg(page.beside.name || "").arg(page.paneName(page.beside.pane || ""))
                            role: "bodySmall"
                            weight: 500
                            maximumLineCount: 1
                        }
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 12
                            height: 12
                            path: Icons.chevronDown
                            color: Theme.surfaceContentVariant
                        }
                    }
                    HoverHandler {
                        id: besideHover
                        cursorShape: Qt.PointingHandCursor
                    }
                    TapHandler {
                        onTapped: besidePill.openMenu()
                    }
                }
                // The thread's menu: continue it on a phone, and its marks.
                IconButton {
                    id: headMore
                    visible: page.isThread && !page.diffShown && !page.prDetailShown && headMenu.actions.length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: Icons.more
                    label: qsTr("More")
                    onClicked: {
                        const p = page.mapFromItem(headMore, headMore.width, headMore.height)
                        headMenu.openAt(p.x, p.y, false, true)
                    }
                }
                // Hides or shows the details beside the thread (Ctrl+]), mirroring the list's toggle.
                IconButton {
                    visible: (page.isThread || page.isSession) && !page.diffShown && !page.prDetailShown
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: page.detailsIcon
                    checked: page.detailsShown
                    label: qsTr("Details")
                    onClicked: page.toggleDetails()
                }
            }
        }
        Rectangle {
            id: headLine
            y: head.height
            width: parent.width
            height: 1
            color: Theme.line
        }

        // ---- A thread or a session ----
        Item {
            id: openView
            visible: (page.isThread || page.isSession || page.isPreview) && !page.diffShown && !page.prDetailShown
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.rightMargin: page.inspectorWidth + page.gitWidth
            onVisibleChanged: transcript.follow = true

            AgentTimelineView {
                id: transcript
                anchors.top: parent.top
                anchors.bottom: bottomBar.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: 16
                anchors.rightMargin: 16
                model: openView.visible ? AgentTimeline : null
                folder: page.info.folder || ""
                now: page.now
                animate: page.active && page.windowActive
                running: page.running
                agentName: page.agentName
                highlightItem: page.highlightItem
                onEditFromHere: (turn) => Agents.editFromHere(turn)
                onRewindTo: (turn) => Agents.rewindOpen(turn)
                onForkFrom: (turn) => Agents.forkTurn(turn)
                onOpenChanges: (turn, path) => Agents.diffOpen(turn, path)
                onUndoRewind: Agents.undoRewind()
                onLinkActivated: (key, index) => linkSheet.ask(key, index)
                onWaitingClicked: page.questioning ? questionDock.pulse() : dock.pulse()
                onImplementPlan: (itemId, mode) => Agents.implement(itemId, mode)
                onImplementMenuRequested: (itemId, x, y) => {
                    page.planItem = itemId
                    const p = page.mapFromItem(null, x, y)
                    planMenu.openAt(p.x, p.y - 30, true, true)
                }
                onChangePlan: composer.focusInput()
                // Where a phone opens it when it's handed over: the first
                // message on screen, or the newest while it follows them.
                onReadingAt: (itemId) => Agents.readingAt(itemId)
                Connections {
                    target: Agents
                    function onOpenIdChanged() { transcript.follow = true }
                }

                header: Column {
                    width: transcript.width
                    topPadding: 10
                    bottomPadding: 6
                    spacing: 8
                    Rectangle {
                        visible: page.isSession
                        width: parent.width
                        height: visible ? bannerRow.height + 16 : 0
                        radius: Theme.radiusMd
                        color: Theme.surfaceContainerLow
                        Row {
                            id: bannerRow
                            x: 12
                            y: 8
                            width: parent.width - 24
                            spacing: 8
                            Icon {
                                width: 16
                                height: 16
                                path: Icons.info
                                color: Theme.surfaceContentVariant
                            }
                            Txt {
                                width: parent.width - 24
                                text: page.bannerText
                                role: "bodySmall"
                                muted: true
                                wrapMode: Text.WordWrap
                                elide: Text.ElideNone
                            }
                        }
                    }
                    TextAction {
                        visible: Agents.more
                        enabled: !Agents.loading
                        text: qsTr("Load earlier")
                        iconPath: Icons.history
                        onClicked: Agents.loadEarlier()
                    }
                    Row {
                        visible: Agents.loading
                        spacing: 8
                        Spinner {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 16
                            height: 16
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Loading…")
                            role: "bodySmall"
                            muted: true
                        }
                    }
                }

                footer: Column {
                    width: transcript.width
                    topPadding: 8
                    bottomPadding: 12
                    spacing: 10
                    // A failed thread: "Failed" (or the usage limit) and the CLI's own
                    // message as it came.
                    Column {
                        id: failure
                        readonly property bool limit: page.info.error_code === "usage_limit"
                        visible: page.isThread && page.phase === "failed"
                        width: parent.width
                        spacing: 4
                        Row {
                            x: 20
                            spacing: 8
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 16
                                height: 16
                                path: failure.limit ? Icons.history : Icons.warning
                                color: failure.limit ? AgentPalette.needsYou : AgentPalette.failed
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: failure.limit ? qsTr("Usage limit reached") : qsTr("Failed")
                                role: "bodySmall"
                                weight: 600
                                color: failure.limit ? AgentPalette.needsYou : AgentPalette.failedText
                            }
                        }
                        Txt {
                            visible: (page.info.error || "").length > 0
                            x: 44
                            width: parent.width - x
                            text: page.info.error || ""
                            role: "code"
                            muted: true
                            wrapMode: Text.WrapAnywhere
                            elide: Text.ElideNone
                        }
                    }
                    // A session that waits in the app it runs in.
                    Rectangle {
                        visible: page.isSession && page.phase === "needs_you"
                        width: parent.width
                        height: visible ? waitColumn.height + 24 : 0
                        radius: Theme.radiusMd
                        color: Theme.surfaceContainerLow
                        border.width: 1
                        border.color: page.info.guess ? Theme.outlineVariant : AgentPalette.needsYou
                        Column {
                            id: waitColumn
                            x: 12
                            y: 12
                            width: parent.width - 24
                            spacing: 10
                            Item {
                                width: parent.width
                                height: Math.max(waitTitle.height, 20)
                                Txt {
                                    id: waitTitle
                                    anchors.left: parent.left
                                    anchors.right: askedAgo.left
                                    anchors.rightMargin: 8
                                    text: page.waitingText
                                    weight: 500
                                    wrapMode: Text.WordWrap
                                    elide: Text.ElideNone
                                }
                                Txt {
                                    id: askedAgo
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    text: qsTr("Asked %1 ago").arg(page.span(page.since))
                                    role: "bodySmall"
                                    muted: true
                                }
                            }
                            Rectangle {
                                visible: !!page.runningStep && (page.runningStep.detail || "").length > 0
                                width: parent.width
                                height: visible ? waitDetail.height + 16 : 0
                                radius: Theme.radiusSm
                                color: Theme.surfaceContainerHigh
                                Txt {
                                    id: waitDetail
                                    x: 10
                                    y: 8
                                    width: parent.width - 20
                                    text: page.runningStep ? (page.runningStep.detail || "") : ""
                                    role: "code"
                                    wrapMode: Text.WrapAnywhere
                                    elide: Text.ElideNone
                                    maximumLineCount: 6
                                }
                            }
                            Txt {
                                width: parent.width
                                text: qsTr("G7 can't answer it from here.")
                                role: "bodySmall"
                                muted: true
                            }
                        }
                    }
                }
            }

            // ---- Under the timeline: the plan, the request, the reply box; or Continue here ----
            Column {
                id: bottomBar
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.leftMargin: 16
                anchors.rightMargin: 16
                bottomPadding: 6
                spacing: 8

                // The agent's plan, pinned here while Details is closed.
                Column {
                    id: planLine
                    readonly property var plan: page.info.plan || []
                    readonly property int done: plan.filter((s) => s.status === "completed").length
                    readonly property var current: plan.find((s) => s.status === "in_progress")
                        || plan.find((s) => s.status !== "completed") || null
                    property bool expanded: false
                    visible: page.isThread && plan.length > 0 && !inspector.visible
                    width: parent.width
                    spacing: 2
                    Repeater {
                        model: planLine.expanded ? planLine.plan : []
                        delegate: Item {
                            id: pinnedStep
                            required property var modelData
                            width: planLine.width
                            height: Math.max(22, pinnedText.height + 4)
                            Icon {
                                visible: pinnedStep.modelData.status === "completed"
                                x: 4
                                y: 4
                                width: 14
                                height: 14
                                path: Icons.check
                                color: AgentPalette.done
                            }
                            Rectangle {
                                visible: pinnedStep.modelData.status !== "completed"
                                x: 7
                                y: 7
                                width: 8
                                height: 8
                                radius: 4
                                color: "transparent"
                                // The current step: a blue ring.
                                border.width: pinnedStep.modelData.status === "in_progress" ? 1.8 : 1.2
                                border.color: pinnedStep.modelData.status === "in_progress" ? AgentPalette.working : Theme.outline
                            }
                            Txt {
                                id: pinnedText
                                x: 26
                                y: 2
                                width: parent.width - x
                                text: pinnedStep.modelData.text
                                role: "bodySmall"
                                muted: pinnedStep.modelData.status === "completed"
                                wrapMode: Text.WordWrap
                                elide: Text.ElideNone
                            }
                        }
                    }
                    Item {
                        width: parent.width
                        height: 26
                        Accessible.role: Accessible.Button
                        Accessible.name: pinnedPlanText.text
                        Accessible.onPressAction: planLine.expanded = !planLine.expanded
                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 6
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                                path: planLine.expanded ? Icons.chevronDown : Icons.chevronUp
                                color: Theme.surfaceContentVariant
                            }
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 15
                                height: 15
                                path: Icons.clipboardList
                                color: Theme.surfaceContentVariant
                            }
                        }
                        Txt {
                            id: pinnedPlanText
                            x: 42
                            width: parent.width - x
                            anchors.verticalCenter: parent.verticalCenter
                            text: planLine.current
                                ? qsTr("Tasks %1/%2 · %3").arg(planLine.done).arg(planLine.plan.length).arg(planLine.current.text)
                                : qsTr("Tasks %1/%2").arg(planLine.done).arg(planLine.plan.length)
                            role: "bodySmall"
                            maximumLineCount: 1
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: planLine.expanded = !planLine.expanded
                        }
                    }
                }

                // A usage window near its end: pinned, never a row of the timeline.
                Row {
                    visible: page.isThread && page.limitText.length > 0
                    width: parent.width
                    spacing: 8
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        path: page.limitTold ? Icons.bell : Icons.history
                        color: AgentPalette.needsYou
                    }
                    Txt {
                        width: parent.width - 22
                        anchors.verticalCenter: parent.verticalCenter
                        text: page.limitText
                        role: "bodySmall"
                        color: AgentPalette.needsYou
                        maximumLineCount: 1
                    }
                }

                // A dev server the thread started: open it here, or on a phone
                // when it answers on the network (§V9.2).
                Rectangle {
                    id: serverLine
                    visible: page.serverPort > 0
                    width: parent.width
                    height: visible ? serverColumn.implicitHeight + 16 : 0
                    radius: Theme.radiusMd
                    color: Theme.raised
                    Column {
                        id: serverColumn
                        x: 12
                        y: 8
                        width: parent.width - 24
                        spacing: 2
                        Item {
                            width: parent.width
                            height: Math.max(serverText.implicitHeight, serverActions.implicitHeight)
                            Icon {
                                id: serverGlyph
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                width: 15
                                height: 15
                                path: page.serverIcon
                                color: Theme.surfaceContentVariant
                            }
                            Txt {
                                id: serverText
                                anchors.left: serverGlyph.right
                                anchors.leftMargin: 8
                                anchors.right: serverActions.left
                                anchors.rightMargin: 8
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("A server is listening on port %1").arg(page.serverPort)
                                role: "bodySmall"
                                maximumLineCount: 1
                            }
                            Row {
                                id: serverActions
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 2
                                Button {
                                    variant: "text"
                                    size: "sm"
                                    text: qsTr("Open in browser")
                                    onClicked: Agents.openServer(page.serverPort)
                                }
                                Repeater {
                                    model: page.server.lan === true ? (page.server.phones || []) : []
                                    delegate: Button {
                                        required property var modelData
                                        variant: "text"
                                        size: "sm"
                                        text: qsTr("Open on %1").arg(modelData.name || "")
                                        onClicked: Agents.openServerOn(page.serverPort, modelData.id || "")
                                    }
                                }
                            }
                        }
                        // Bound to this PC's loopback: said, not tunnelled.
                        Txt {
                            visible: page.server.lan === false
                            x: 23
                            width: parent.width - x
                            text: qsTr("It only answers on this PC. Start it with --host to open it on a phone.")
                            role: "bodySmall"
                            muted: true
                            wrapMode: Text.WordWrap
                            elide: Text.ElideNone
                        }
                    }
                }

                // The question it waits on: one ask at a time, at most half the column.
                AgentQuestionDock {
                    id: questionDock
                    function pulse() { questionPulse.restart() }
                    visible: page.questioning
                    width: parent.width
                    height: visible ? Math.min(implicitHeight, openView.height / 2) : 0
                    question: page.question
                    busy: Agents.busy
                    onPicked: (index) => Agents.questionPick(index)
                    onBack: Agents.questionStep(false)
                    onNext: Agents.questionStep(true)
                    onSend: Agents.questionSend()
                    onSecretTyped: (text) => Agents.questionSecret(text)
                    onOverflowRequested: (x, y) => {
                        const p = page.mapFromItem(null, x, y)
                        dockMenu.openAt(p.x, p.y - 30, true, true)
                    }
                    SequentialAnimation on opacity {
                        id: questionPulse
                        running: false
                        NumberAnimation { to: 0.55; duration: Theme.reduceMotion ? 0 : 120 }
                        NumberAnimation { to: 1; duration: Theme.reduceMotion ? 0 : 240 }
                    }
                }

                // The request it waits on: one at a time, at most half the column.
                AgentDock {
                    id: dock
                    function pulse() { pulseAnim.restart() }
                    visible: page.asking
                    width: parent.width
                    height: visible ? Math.min(implicitHeight, openView.height / 2) : 0
                    ask: page.ask
                    agentName: page.agentName
                    folder: page.info.folder || ""
                    alwaysOffered: page.ask.can_allow_thread === true && page.info.mode !== "auto_edit"
                        && (page.ask.kind === "command" || page.ask.kind === "edit")
                    composerHasText: composer.hasText
                    busy: Agents.busy
                    now: page.now
                    onAllow: Agents.answer(page.ask.token, "allow")
                    onDeny: Agents.answer(page.ask.token, "deny")
                    onAllowThread: Agents.answer(page.ask.token, "allow_thread")
                    onStop: Agents.stop()
                    onOverflowRequested: (x, y) => {
                        const p = page.mapFromItem(null, x, y)
                        dockMenu.openAt(p.x, p.y - 30, true, true)
                    }
                    SequentialAnimation on opacity {
                        id: pulseAnim
                        running: false
                        NumberAnimation { to: 0.55; duration: Theme.reduceMotion ? 0 : 120 }
                        NumberAnimation { to: 1; duration: Theme.reduceMotion ? 0 : 240 }
                    }
                }

                // Read only: its folder isn't one of your agent folders.
                Rectangle {
                    visible: page.readOnly
                    width: parent.width
                    height: visible ? Math.max(readOnlyText.height, readOnlyButton.height) + 24 : 0
                    radius: Theme.radiusMd
                    color: "transparent"
                    border.width: 1
                    border.color: Theme.outlineVariant
                    Icon {
                        id: readOnlyIcon
                        x: 12
                        anchors.verticalCenter: parent.verticalCenter
                        width: 16
                        height: 16
                        path: Icons.folder
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        id: readOnlyText
                        anchors.left: readOnlyIcon.right
                        anchors.leftMargin: 12
                        anchors.right: readOnlyButton.left
                        anchors.rightMargin: 12
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Add %1 to your agent folders to continue it.").arg(page.info.folder || "")
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                    Button {
                        id: readOnlyButton
                        anchors.right: parent.right
                        anchors.rightMargin: 12
                        anchors.verticalCenter: parent.verticalCenter
                        variant: "tonal"
                        size: "sm"
                        iconPath: Icons.plus
                        text: qsTr("Add folder…")
                        onClicked: page.addFolder("")
                    }
                }

                // A session found on this PC, looked at before it's brought in.
                Item {
                    visible: page.isPreview
                    width: parent.width
                    height: visible ? 52 : 0
                    Txt {
                        anchors.left: parent.left
                        anchors.right: previewButton.left
                        anchors.rightMargin: 12
                        anchors.verticalCenter: parent.verticalCenter
                        text: page.info.brought_in === true ? qsTr("Brought in") : qsTr("A preview. Nothing is brought in yet.")
                        role: "bodySmall"
                        muted: true
                        maximumLineCount: 1
                    }
                    Button {
                        id: previewButton
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        iconPath: Icons.download
                        text: qsTr("Bring in %n", "", 1)
                        enabled: page.info.selectable === true && (page.importState.job || {}).total === undefined
                        busy: Agents.busy
                        onClicked: page.bringInPreview()
                    }
                }

                AgentComposer {
                    id: composer
                    visible: page.isThread && !page.readOnly
                    width: parent.width
                    height: visible ? implicitHeight : 0
                    ask: page.ask
                    question: page.question
                    controls: page.composerState
                    popup: page.popup
                    setText: page.composerSet
                    canSayWhy: Agents.canSayWhy
                    running: page.running
                    busy: Agents.busy
                    agentName: page.agentName
                    agentId: page.info.agent || ""
                    folder: page.info.folder || ""
                    quotes: page.gitState.quotes || []
                    // The message and what answers it show, however far up the transcript was.
                    onSend: (text, other) => {
                        Agents.send(text, other)
                        transcript.jumpToLatest()
                    }
                    onDenySend: (token, text) => Agents.denyWithMessage(token, text)
                    onAllow: page.allowShown()
                    onStop: Agents.stop()
                    onAttachRequested: (x, y) => {
                        const p = page.mapFromItem(null, x, y)
                        attachMenu.openAt(p.x, p.y, true, false)
                    }
                    onPickerRequested: (x, y) => {
                        const p = page.mapFromItem(null, x, y)
                        threadPicker.openAt(p.x, p.y)
                    }
                    onModeMenuRequested: (x, y) => {
                        const p = page.mapFromItem(null, x, y)
                        modeMenu.openAt(p.x, p.y, true, false)
                    }
                }
                Column {
                    id: sessionBar
                    visible: page.isSession
                    width: parent.width
                    spacing: 8
                    readonly property bool ended: page.phase === "ended"
                    readonly property string blocked: page.info.blocked || ""

                    // Ended: it can become a thread here.
                    Rectangle {
                        visible: sessionBar.ended
                        width: parent.width
                        height: visible ? endedColumn.height + 24 : 0
                        radius: Theme.radiusMd
                        color: Theme.surfaceContainerLow
                        border.width: 1
                        border.color: Theme.outlineVariant
                        Column {
                            id: endedColumn
                            x: 12
                            y: 12
                            width: parent.width - 24
                            spacing: 8
                            Txt {
                                width: parent.width
                                text: qsTr("This session ended %1 ago.").arg(page.span(page.info.updated_ms || 0))
                                weight: 500
                                wrapMode: Text.WordWrap
                                elide: Text.ElideNone
                            }
                            Txt {
                                width: parent.width
                                text: sessionBar.blocked === "folder"
                                    ? qsTr("Add %1 to your agent folders to continue it here.").arg(page.info.folder || "")
                                    : sessionBar.blocked === "agent"
                                        ? qsTr("%1 isn't ready on this PC. Check it in Setup.").arg(page.agentName)
                                        : qsTr("Continue it here and it becomes a G7 thread in %1. Your phone can then follow it and answer it.").arg(page.info.folder || "")
                                role: "bodySmall"
                                muted: true
                                wrapMode: Text.WordWrap
                                elide: Text.ElideNone
                            }
                            Flow {
                                width: parent.width
                                spacing: 8
                                layoutDirection: Qt.RightToLeft
                                Button {
                                    visible: sessionBar.blocked !== "folder" && sessionBar.blocked !== "agent"
                                    text: qsTr("Continue here")
                                    busy: Agents.busy
                                    enabled: page.info.can_continue === true
                                    onClicked: continueSheet.open()
                                }
                                Button {
                                    visible: sessionBar.blocked === "folder" && (page.info.cwd || "").length > 0
                                    iconPath: Icons.plus
                                    text: qsTr("Add folder…")
                                    onClicked: folderSheet.ask(page.info.cwd, page.info.id)
                                }
                                Button {
                                    visible: sessionBar.blocked === "agent"
                                    variant: "tonal"
                                    iconPath: Icons.settings
                                    text: qsTr("Setup")
                                    onClicked: page.openSetup()
                                }
                            }
                        }
                    }

                    // Still open somewhere: why it can't continue yet.
                    Item {
                        visible: !sessionBar.ended
                        width: parent.width
                        height: visible ? Math.max(openReason.height, continueLater.height) : 0
                        Txt {
                            id: openReason
                            anchors.left: parent.left
                            anchors.right: continueLater.left
                            anchors.rightMargin: 12
                            anchors.verticalCenter: parent.verticalCenter
                            text: sessionBar.blocked === "folder"
                                ? qsTr("Add %1 to your agent folders to continue it here.").arg(page.info.folder || "")
                                : page.stillOpenText
                            role: "bodySmall"
                            muted: true
                            wrapMode: Text.WordWrap
                            elide: Text.ElideNone
                        }
                        Button {
                            id: continueLater
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            variant: "tonal"
                            size: "sm"
                            text: qsTr("Continue here")
                            enabled: page.info.can_continue === true
                            onClicked: continueSheet.open()
                        }
                    }

                    Row {
                        visible: !sessionBar.ended
                        width: parent.width
                        spacing: 8
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 14
                            height: 14
                            path: Icons.info
                            color: Theme.surfaceContentVariant
                        }
                        Txt {
                            width: parent.width - 22
                            text: page.followingText
                            role: "caption"
                            muted: true
                            maximumLineCount: 2
                            wrapMode: Text.WordWrap
                        }
                    }
                }
            }
        }

        // ---- Details: the plan, the changes, where it runs ----
        Rectangle {
            id: inspector
            readonly property bool overlay: page.detailsOpen && !page.inspectorFits
            visible: (page.isThread || page.isSession) && !page.diffShown && !page.gitShown && !page.runsShown
                     && !page.prDetailShown
                     && (page.inspectorWidth > 0 || overlay)
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            anchors.right: parent.right
            width: overlay ? Math.min(280, mainPane.width - 40) : page.inspectorWidth
            color: Theme.surface
            clip: true
            z: 2
            Rectangle {
                width: 1
                height: parent.height
                color: Theme.line
            }
            Flickable {
                id: inspectorFlick
                // Its content keeps its width while the panel opens and closes.
                x: 1
                width: (inspector.overlay ? inspector.width : 250) - 1
                height: parent.height
                contentHeight: inspectorColumn.height + 16
                boundsBehavior: Flickable.StopAtBounds
                clip: true
                Column {
                    id: inspectorColumn
                    x: 8
                    y: 8
                    width: inspectorFlick.width - 16
                    spacing: 14

                    // The agent's own plan, when it made one.
                    Column {
                        id: planSection
                        readonly property var plan: page.info.plan || []
                        visible: plan.length > 0
                        width: parent.width
                        Item {
                            width: parent.width
                            height: 28
                            Txt {
                                x: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("Plan")
                                role: "label"
                                muted: true
                            }
                            Txt {
                                anchors.right: parent.right
                                anchors.rightMargin: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("%1 of %2 done").arg(planSection.plan.filter((s) => s.status === "completed").length)
                                    .arg(planSection.plan.length)
                                role: "bodySmall"
                                muted: true
                            }
                        }
                        Repeater {
                            model: planSection.plan
                            delegate: Item {
                                id: planStep
                                required property var modelData
                                width: inspectorColumn.width
                                height: Math.max(26, planText.height + 8)
                                Icon {
                                    visible: planStep.modelData.status === "completed"
                                    x: Theme.rowPadding
                                    y: 5
                                    width: 15
                                    height: 15
                                    path: Icons.check
                                    color: AgentPalette.done
                                }
                                Rectangle {
                                    visible: planStep.modelData.status !== "completed"
                                    x: Theme.rowPadding + 3
                                    y: 8
                                    width: 9
                                    height: 9
                                    radius: 4.5
                                    color: "transparent"
                                    // The current step: a blue ring.
                                    border.width: planStep.modelData.status === "in_progress" ? 1.8 : 1.2
                                    border.color: planStep.modelData.status === "in_progress" ? AgentPalette.working : Theme.outline
                                }
                                Txt {
                                    id: planText
                                    x: Theme.rowPadding + 24
                                    y: 4
                                    width: parent.width - x - Theme.rowPadding
                                    text: planStep.modelData.text
                                    role: "bodySmall"
                                    muted: planStep.modelData.status === "completed"
                                    wrapMode: Text.WordWrap
                                    elide: Text.ElideNone
                                }
                            }
                        }
                    }

                    // The files its edits changed.
                    Column {
                        visible: page.changes.length > 0
                        width: parent.width
                        Item {
                            width: parent.width
                            height: 28
                            Txt {
                                x: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("Changes")
                                role: "label"
                                muted: true
                            }
                            Txt {
                                anchors.right: parent.right
                                anchors.rightMargin: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("%n file(s)", "", page.changes.length)
                                role: "bodySmall"
                                muted: true
                            }
                        }
                        Repeater {
                            model: page.changes
                            delegate: Item {
                                id: changeRow
                                required property var modelData
                                width: inspectorColumn.width
                                height: 26
                                Icon {
                                    x: Theme.rowPadding
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 14
                                    height: 14
                                    path: Icons.edit
                                    color: Theme.surfaceContentVariant
                                }
                                Txt {
                                    anchors.left: parent.left
                                    anchors.leftMargin: Theme.rowPadding + 22
                                    anchors.right: counts.left
                                    anchors.rightMargin: 8
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: changeRow.modelData.path
                                    role: "code"
                                    elide: Text.ElideMiddle
                                }
                                Row {
                                    id: counts
                                    anchors.right: parent.right
                                    anchors.rightMargin: Theme.rowPadding
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: 6
                                    Txt {
                                        text: "+" + changeRow.modelData.added
                                        role: "bodySmall"
                                        color: AgentPalette.inserted
                                    }
                                    Txt {
                                        text: "−" + changeRow.modelData.removed
                                        role: "bodySmall"
                                        color: AgentPalette.deleted
                                    }
                                }
                            }
                        }
                    }

                    // How full its context is, as the CLI last reported.
                    Column {
                        visible: page.isThread && page.context.max !== undefined
                        width: parent.width
                        spacing: 4
                        Item {
                            width: parent.width
                            height: 28
                            Txt {
                                x: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                text: qsTr("Context")
                                role: "label"
                                muted: true
                            }
                        }
                        Txt {
                            x: Theme.rowPadding
                            text: qsTr("Context %1% of %2").arg(page.context.pct || 0).arg(page.context.max || "")
                            role: "bodySmall"
                        }
                        Rectangle {
                            x: Theme.rowPadding
                            width: parent.width - 2 * Theme.rowPadding
                            height: 4
                            radius: 2
                            color: Theme.surfaceContainerHigh
                            Rectangle {
                                width: parent.width * Math.min(1, (page.context.pct || 0) / 100)
                                height: parent.height
                                radius: 2
                                color: (page.context.pct || 0) >= 80 ? AgentPalette.needsYou : Theme.surfaceContentVariant
                            }
                        }
                    }

                    // Where it runs and since when.
                    Column {
                        width: parent.width
                        Item {
                            width: parent.width
                            height: 28
                            Txt {
                                x: Theme.rowPadding
                                anchors.verticalCenter: parent.verticalCenter
                                text: page.isSession ? qsTr("Session") : qsTr("Thread")
                                role: "label"
                                muted: true
                            }
                        }
                        Repeater {
                            model: [
                                { icon: Icons.folder, text: page.isSession ? (page.info.shown_cwd || "") : (page.info.folder || "") },
                                { icon: page.isSession || page.info.from === "cli" ? Icons.terminal
                                      : (page.info.from === "phone" ? Icons.phone : Icons.laptop),
                                  text: page.originLine },
                                { icon: Icons.shield, text: page.isSession ? ""
                                      : (page.info.mode === "auto_edit" ? qsTr("Asks before commands")
                                         : page.info.mode === "plan" ? qsTr("Plan")
                                         : qsTr("Asks before edits and commands")) },
                                { icon: Icons.sparkle, text: page.isThread && page.composerState.control === true
                                      ? page.modelLine : "" },
                                { icon: Icons.sparkle, text: page.isThread && (page.info.answered_model || "").length > 0
                                      && (page.info.model || "").length > 0 && page.info.answered_model !== page.info.model
                                      ? qsTr("Answered by %1").arg(page.info.answered_model) : "" },
                                { icon: Icons.history, text: page.clockText },
                                { icon: page.plugIcon, text: page.isThread && page.mcp.on === true && (page.mcp.names || []).length > 0
                                      ? qsTr("%n MCP server(s)", "", page.mcp.names.length) + ": " + page.mcp.names.join(", ") : "" },
                                { icon: Icons.warning, text: page.isThread && (page.mcp.needLogin || 0) > 0
                                      ? qsTr("%n need(s) sign-in. Sign in with %1 in a terminal.", "", page.mcp.needLogin).arg(page.agentName) : "" }
                            ].filter((r) => r.text.length > 0)
                            delegate: Item {
                                id: detailRow
                                required property var modelData
                                width: inspectorColumn.width
                                height: Math.max(26, detailText.height + 8)
                                Icon {
                                    x: Theme.rowPadding
                                    y: 5
                                    width: 15
                                    height: 15
                                    path: detailRow.modelData.icon
                                    color: Theme.surfaceContentVariant
                                }
                                Txt {
                                    id: detailText
                                    x: Theme.rowPadding + 24
                                    y: 4
                                    width: parent.width - x - Theme.rowPadding
                                    text: detailRow.modelData.text
                                    role: "bodySmall"
                                    wrapMode: Text.WordWrap
                                    elide: Text.ElideNone
                                }
                            }
                        }
                    }
                }
                TouchpadGlide { view: inspectorFlick }
            }
            G7ScrollBar {
                flickable: inspectorFlick
                anchors.top: inspectorFlick.top
                anchors.bottom: inspectorFlick.bottom
                anchors.right: inspectorFlick.right
            }
        }

        // ---- Source control, beside the transcript or over it ----
        Rectangle {
            id: gitPane
            readonly property bool overlay: !page.gitInline
            visible: page.gitShown
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            anchors.right: parent.right
            width: overlay ? Math.min(360, mainPane.width - 40) : page.gitWidth
            color: Theme.surface
            clip: true
            z: 3
            Rectangle {
                width: 1
                height: parent.height
                color: Theme.line
            }
            AgentGitPanel {
                x: 1
                width: parent.width - 1
                height: parent.height
                git: page.gitState
                now: page.now
                onClosed: Agents.gitPanel(false)
                onBranchMenuAt: (x, y) => {
                    const p = page.mapFromItem(null, x, y)
                    branchMenu.anchorX = p.x
                    branchMenu.anchorY = p.y
                    Agents.gitBranchMenu(true)
                }
                onSwitchAsked: (branch, files) => switchDialog.ask(branch, files)
                onPushMenuAt: (x, y, actions) => {
                    const p = page.mapFromItem(null, x, y)
                    gitMenu.actions = actions
                    gitMenu.openAt(p.x, p.y, false, false)
                }
                onRemoveWorktree: (branch, unpushed, dirty) => worktreeDialog.ask("", branch, unpushed, dirty)
                onOpenFile: (path) => Agents.diffUncommitted(path)
            }
        }

        // ---- The run panel, in the same slot (§X5.1) ----
        Rectangle {
            id: runPane
            readonly property bool overlay: !page.gitInline
            visible: page.runsShown
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            anchors.right: parent.right
            width: overlay ? Math.min(360, mainPane.width - 40) : page.gitWidth
            color: Theme.surface
            clip: true
            z: 3
            Rectangle {
                width: 1
                height: parent.height
                color: Theme.line
            }
            AgentRunPanel {
                x: 1
                width: parent.width - 1
                height: parent.height
                runs: page.runsState
                now: page.now
                onClosed: Agents.runPanel(false)
            }
        }

        // ---- Pull requests ----
        AgentPrList {
            visible: page.shown === "prs" && !page.prDetailShown && !page.diffShown
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            width: parent.width
            prs: page.prsState
            folders: page.setup.folders || []
            now: page.now
            onOpenThread: (id) => page.openRow("thread", id)
            onOpenDetail: (repo, number) => Agents.prDetailOpen(repo, number)
        }

        // ---- A pull request, read-only (§X6.6) ----
        AgentPrDetail {
            visible: page.prDetailShown
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            width: parent.width
            z: 4
            detail: page.prDetailState
            now: page.now
            onLinkActivated: (key, index) => linkSheet.ask(key, index)
            onAskInNewThread: (repo, number, folderId) => page.askInNewThread(repo, number, folderId)
        }

        // ---- A new thread ----
        Flickable {
            id: newView
            visible: page.shown === "new"
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            width: parent.width
            contentHeight: form.height + 32
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            property string agent: page.readyAgents.length > 0 ? page.readyAgents[0].id : ""
            property string folder: page.usableFolders.length > 0 ? page.usableFolders[0].id : ""
            property string mode: "supervised"
            readonly property var agentInfo: page.readyAgents.find((a) => a.id === newView.agent) || ({})
            // Plan, when the host and the agent can.
            readonly property bool planAvailable: page.newPicker.control === true && page.newPicker.plan === true
            onAgentChanged: Agents.newForm(newView.agent, newView.folder)
            onFolderChanged: Agents.newForm(newView.agent, newView.folder)
            Component.onCompleted: Agents.newForm(newView.agent, newView.folder)
            onPlanAvailableChanged: if (!planAvailable && mode === "plan") mode = "supervised"
            function start() {
                // "Ask in a new thread…": the host writes the review message itself.
                if (page.reviewing) {
                    if (newView.agent.length === 0 || newView.folder.length === 0)
                        return
                    Agents.prReviewNewThread(newView.agent, newView.folder, newView.mode,
                                             page.reviewChip.repo || "", page.reviewChip.number || 0)
                    return
                }
                const task = taskInput.text.trim()
                if (task.length === 0 || newView.agent.length === 0 || newView.folder.length === 0)
                    return
                if (worktreeFields.worktree) {
                    if (!worktreeFields.branchOk)
                        return
                    Agents.startWorktree(newView.agent, newView.folder, task, newView.mode,
                                         worktreeFields.branch, worktreeFields.base)
                } else {
                    Agents.start(newView.agent, newView.folder, task, newView.mode)
                }
                taskInput.text = ""
            }

            Column {
                id: form
                x: 16
                y: 16
                width: Math.min(720, newView.width - 32)
                spacing: 16

                // In place of the prompt: the pull request this thread reviews.
                Row {
                    visible: page.reviewing
                    spacing: 6
                    Chip {
                        anchors.verticalCenter: parent.verticalCenter
                        iconPath: AgentGitGlyphs.pullRequest
                        text: qsTr("Review pull request #%1").arg(page.reviewChip.number || 0)
                        selected: true
                    }
                    IconButton {
                        anchors.verticalCenter: parent.verticalCenter
                        iconPath: Icons.close
                        iconSize: 14
                        label: qsTr("Remove")
                        onClicked: page.reviewChip = ({})
                    }
                }
                Rectangle {
                    visible: !page.reviewing
                    width: parent.width
                    height: Math.min(Math.max(taskInput.contentHeight + 24, 110), 260)
                    radius: Theme.rowRadius
                    color: Theme.raised
                    border.width: taskInput.activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                    Flickable {
                        id: taskScroll
                        anchors.fill: parent
                        anchors.margins: 12
                        contentHeight: taskInput.contentHeight
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        TextEdit {
                            id: taskInput
                            width: taskScroll.width
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            font.family: Theme.fontUi
                            font.pixelSize: Theme.typography.body.size
                            color: Theme.surfaceContent
                            selectionColor: Theme.selection
                            selectedTextColor: Theme.surfaceContent
                            Accessible.name: qsTr("What should it do?")
                            // Ctrl+Enter starts it; Enter starts a new line.
                            Keys.onPressed: (event) => {
                                if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                                    && (event.modifiers & Qt.ControlModifier)) {
                                    newView.start()
                                    event.accepted = true
                                }
                            }
                            onCursorRectangleChanged: {
                                if (cursorRectangle.y + cursorRectangle.height > taskScroll.contentY + taskScroll.height)
                                    taskScroll.contentY = cursorRectangle.y + cursorRectangle.height - taskScroll.height
                            }
                            Txt {
                                visible: taskInput.text.length === 0
                                width: taskInput.width
                                text: qsTr("What should it do?")
                                muted: true
                            }
                        }
                    }
                }

                Column {
                    width: parent.width
                    spacing: 6
                    visible: page.readyAgents.length > 0
                    Txt {
                        text: qsTr("Agent")
                        role: "label"
                        muted: true
                    }
                    Segmented {
                        options: page.readyAgents.map((a) => ({ value: a.id, label: a.name }))
                        value: newView.agent
                        onPicked: (value) => { newView.agent = value }
                    }
                }

                Column {
                    width: parent.width
                    spacing: 6
                    visible: page.usableFolders.length > 0
                    Txt {
                        text: qsTr("Folder")
                        role: "label"
                        muted: true
                    }
                    Flow {
                        width: parent.width
                        spacing: 6
                        Repeater {
                            model: page.usableFolders
                            delegate: Chip {
                                id: folderChip
                                required property var modelData
                                interactive: true
                                iconPath: Icons.folder
                                text: modelData.label
                                selected: newView.folder === modelData.id
                                onClicked: newView.folder = folderChip.modelData.id
                            }
                        }
                    }
                }

                // Work in this folder, or in a new worktree on its own branch.
                AgentWorktreeFields {
                    id: worktreeFields
                    visible: !page.reviewing
                    width: parent.width
                    folderId: newView.folder
                    gitSetup: page.gitSetup
                    words: taskInput.text
                    onBasesMenu: (x, y, actions) => {
                        const p = page.mapFromItem(null, x, y)
                        gitMenu.actions = actions
                        gitMenu.openAt(p.x, p.y, false, false)
                    }
                }

                Column {
                    width: parent.width
                    spacing: 6
                    Txt {
                        text: qsTr("Asks before")
                        role: "label"
                        muted: true
                    }
                    Segmented {
                        options: [
                            { value: "supervised", label: qsTr("Edits and commands") },
                            { value: "auto_edit", label: qsTr("Commands only") }
                        ].concat(newView.planAvailable ? [{ value: "plan", label: qsTr("Plan") }] : [])
                        value: newView.mode
                        onPicked: (value) => { newView.mode = value }
                    }
                    Txt {
                        width: parent.width
                        text: newView.mode === "auto_edit"
                            ? qsTr("It changes files without asking, and asks before a command that could change something.")
                            : newView.mode === "plan"
                                ? qsTr("Plan: it changes nothing until you say so.")
                                : qsTr("It asks before it changes a file or runs a command that could change something.")
                        role: "bodySmall"
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                }

                // The model and effort it starts with, from the agent's own list.
                Column {
                    width: parent.width
                    spacing: 6
                    visible: page.newPicker.control === true
                    Txt {
                        text: qsTr("Model")
                        role: "label"
                        muted: true
                    }
                    Chip {
                        id: newModelChip
                        interactive: true
                        iconPath: Icons.sparkle
                        text: {
                            const label = page.newPicker.label
                            if (!label || label.model.length === 0)
                                return label && label.effort.length > 0 ? qsTr("Default") + " · " + label.effort : qsTr("Default")
                            return label.effort.length > 0 ? label.model + " · " + label.effort : label.model
                        }
                        onClicked: {
                            const p = newModelChip.mapToItem(page, 0, 0)
                            newFormPicker.openAt(p.x, p.y)
                        }
                    }
                }

                Item {
                    width: parent.width
                    height: startButton.height
                    Txt {
                        visible: !page.ready
                        anchors.left: parent.left
                        anchors.right: startButton.left
                        anchors.rightMargin: 12
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Set up an agent and a folder first.")
                        role: "bodySmall"
                        muted: true
                        wrapMode: Text.WordWrap
                    }
                    Button {
                        id: startButton
                        anchors.right: parent.right
                        text: qsTr("Start")
                        busy: Agents.busy || (page.gitSetup.creating || {}).running === true
                        enabled: page.ready && (page.reviewing || (taskInput.text.trim().length > 0
                                 && (!worktreeFields.worktree || worktreeFields.branchOk)))
                        onClicked: newView.start()
                    }
                }
            }
            TouchpadGlide { view: newView }
        }
        G7ScrollBar {
            flickable: newView
            anchors.top: newView.top
            anchors.bottom: newView.bottom
            anchors.right: newView.right
        }

        // ---- What a turn, or the whole thread, changed ----
        Item {
            id: diffView
            readonly property var files: page.diffState.files || []
            readonly property var file: files[page.diffState.selected || 0] || ({})
            readonly property var patch: page.diffState.patch || ({})
            readonly property bool narrowList: width < 720
            visible: page.diffShown
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            width: parent.width

            AgentDiffFiles {
                id: diffFiles
                visible: !diffView.narrowList && diffView.files.length > 0
                width: visible ? 250 : 0
                height: parent.height
                files: diffView.files
                selected: page.diffState.selected || 0
                more: page.diffState.more || 0
                onPicked: (index) => Agents.diffSelect(index)
            }
            Rectangle {
                visible: diffFiles.visible
                x: diffFiles.width - 1
                width: 1
                height: parent.height
                color: Theme.line
            }
            Item {
                id: diffBody
                anchors.left: diffFiles.right
                anchors.right: parent.right
                height: parent.height
                // The file's name and its lines added and removed; on a narrow
                // page the name opens the list of files.
                Item {
                    id: fileHead
                    visible: diffView.files.length > 0
                    width: parent.width
                    height: visible ? 36 : 0
                    Item {
                        id: filePicker
                        x: 12
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(fileName.implicitWidth + (diffView.narrowList ? 24 : 0), fileHead.width - 120)
                        height: 28
                        Txt {
                            id: fileName
                            anchors.verticalCenter: parent.verticalCenter
                            width: Math.min(implicitWidth, parent.width - (diffView.narrowList ? 24 : 0))
                            text: (diffView.file.from || "").length > 0
                                ? diffView.file.from + " → " + diffView.file.path : (diffView.file.path || "")
                            role: "code"
                            size: Theme.typography.bodySmall.size
                            elide: Text.ElideLeft
                        }
                        Icon {
                            visible: diffView.narrowList
                            anchors.left: fileName.right
                            anchors.leftMargin: 6
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13
                            height: 13
                            path: Icons.chevronDown
                            color: Theme.surfaceContentVariant
                        }
                        MouseArea {
                            anchors.fill: parent
                            enabled: diffView.narrowList
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                const p = page.mapFromItem(filePicker, 0, filePicker.height)
                                diffMenu.openAt(p.x, p.y, false, false)
                            }
                        }
                    }
                    Row {
                        visible: !diffView.file.binary && diffView.file.status !== "unrecorded"
                        anchors.right: parent.right
                        anchors.rightMargin: 12
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Txt {
                            text: "+" + (diffView.file.adds || 0)
                            role: "bodySmall"
                            color: AgentPalette.inserted
                        }
                        Txt {
                            text: "−" + (diffView.file.dels || 0)
                            role: "bodySmall"
                            color: AgentPalette.deleted
                        }
                    }
                    Rectangle {
                        anchors.bottom: parent.bottom
                        width: parent.width
                        height: 1
                        color: Theme.line
                    }
                }
                AgentDiffView {
                    id: diffLines
                    visible: (diffView.patch.doc || "").length > 0 && !diffView.patch.loading && !diffView.patch.binary
                             && !diffView.patch.unrecorded && !page.diffState.loading
                    anchors.top: fileHead.bottom
                    anchors.topMargin: 6
                    anchors.bottom: cutLine.top
                    width: parent.width
                    paged: true
                    showPaths: false
                    diff: diffView.patch.doc || ""
                    // Lines picked here become a quote chip in the composer.
                    selectable: page.gitOn
                    onComment: (first, last) => Agents.quoteAdd(first, last)
                }
                // Loading, binary, not recorded, nothing to show.
                Column {
                    visible: !diffLines.visible
                    anchors.centerIn: parent
                    spacing: 8
                    Spinner {
                        visible: page.diffState.loading === true || diffView.patch.loading === true
                        anchors.horizontalCenter: parent.horizontalCenter
                        width: 18
                        height: 18
                    }
                    Icon {
                        visible: diffView.patch.binary === true
                        anchors.horizontalCenter: parent.horizontalCenter
                        width: 20
                        height: 20
                        path: Icons.photo
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: page.diffState.loading === true || diffView.patch.loading === true ? qsTr("Loading…")
                            : diffView.patch.binary === true ? qsTr("Binary file")
                            : diffView.patch.unrecorded === true ? qsTr("%n files weren't recorded (too large).", "", 1)
                            : page.diffState.unavailable === true ? qsTr("Changes weren't recorded for this turn.")
                            : diffView.patch.failed === true ? qsTr("Something went wrong. Try again.") : ""
                        role: "bodySmall"
                        muted: true
                    }
                }
                Item {
                    id: cutLine
                    anchors.bottom: parent.bottom
                    width: parent.width
                    height: diffView.patch.truncated === true && diffLines.visible ? 32 : 0
                    visible: height > 0
                    Rectangle {
                        width: parent.width
                        height: 1
                        color: Theme.line
                    }
                    Row {
                        x: 12
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 8
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 14
                            height: 14
                            path: Icons.info
                            color: Theme.surfaceContentVariant
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Showing the first 1 MB")
                            role: "bodySmall"
                            muted: true
                        }
                    }
                }
            }
        }

        // ---- Bring in earlier sessions ----
        AgentImport {
            id: importView
            visible: page.shown === "import"
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            width: parent.width
            importState: page.importState
            busy: Agents.busy
            onPreviewed: (id) => Agents.previewOpen(id)
            onVisibleChanged: if (visible) Qt.callLater(() => importView.focusSearch())
        }

        // ---- Setup ----
        Flickable {
            id: setupView
            visible: page.shown === "setup"
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            width: parent.width
            contentHeight: setupArea.height + 32
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            AgentSetup {
                id: setupArea
                x: Theme.rowPadding
                y: 12
                width: Math.min(960, setupView.width - 2 * Theme.rowPadding)
                setup: page.setup
                gitSetup: page.gitSetup
                onAddFolderRequested: page.addFolder("")
                onImportRequested: page.openImport()
                onRemoveWorktree: (id, branch, unpushed) => worktreeDialog.ask(id, branch, unpushed, 0)
            }
            TouchpadGlide { view: setupView }
        }
        G7ScrollBar {
            flickable: setupView
            anchors.top: setupView.top
            anchors.bottom: setupView.bottom
            anchors.right: setupView.right
        }
    }

    // ---- Sheets ----

    // Asks before agents may work in a folder: it shows the whole path.
    Sheet {
        id: folderSheet
        cardWidth: 460
        property string path: ""
        // A session to continue once it's added ("" for none).
        property string session: ""
        function ask(path, session) {
            if (path.length === 0)
                return
            folderSheet.path = path
            folderSheet.session = session || page.continueAfterAdd
            page.continueAfterAdd = ""
            open()
        }
        readonly property string name: {
            const parts = path.split("/").filter((p) => p.length > 0)
            return parts.length > 0 ? parts[parts.length - 1] : path
        }
        Column {
            width: parent.width
            spacing: 12
            Txt {
                width: parent.width
                text: qsTr("Let agents work in %1?").arg(folderSheet.name)
                role: "headline"
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Txt {
                width: parent.width
                text: folderSheet.path
                role: "code"
                wrapMode: Text.WrapAnywhere
                elide: Text.ElideNone
            }
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
                role: "body"
                muted: true
                text: qsTr("Agents can read, change and run things in this folder, and a command you approve can reach outside it. The folder's own agent settings, such as hooks, also run, so add it only if you trust it. Your phone sees only its name.")
            }
            Txt {
                visible: page.setup.control === true
                width: parent.width
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
                role: "body"
                muted: true
                text: (page.setup.agents ?? []).some((a) => a.id === "codex" && a.mcpSwitch === true)
                    ? qsTr("The MCP servers you set up for Claude and Codex are used here too; you can turn that off for this folder.")
                    : qsTr("The MCP servers you set up for Claude and Codex are used here too; you can turn that off for Claude in this folder.")
            }
            Txt {
                visible: page.undoOn
                width: parent.width
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
                role: "body"
                muted: true
                text: qsTr("G7 keeps a restore point for each turn in its own folder on this PC; you can turn that off for this folder.")
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Cancel")
                    onClicked: folderSheet.close()
                }
                Button {
                    iconPath: Icons.plus
                    text: folderSheet.session.length > 0 ? qsTr("Add and continue") : qsTr("Add folder")
                    onClicked: {
                        if (folderSheet.session.length > 0)
                            Agents.addFolderAndContinue(folderSheet.path, folderSheet.session)
                        else
                            Agents.addFolder(folderSheet.path)
                        folderSheet.close()
                    }
                }
            }
        }
    }

    // Continue a session here: what that does, and what not to do meanwhile.
    Sheet {
        id: continueSheet
        cardWidth: 460
        Column {
            width: parent.width
            spacing: 12
            Txt {
                width: parent.width
                text: qsTr("Continue this session in G7?")
                role: "headline"
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Txt {
                width: parent.width
                text: qsTr("G7 copies it into a new thread in %1, with %2. Nothing runs until you send a message.")
                    .arg(page.info.folder || "").arg(page.agentName)
                role: "body"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Row {
                width: parent.width
                spacing: 8
                Icon {
                    width: 16
                    height: 16
                    path: Icons.shield
                    color: Theme.surfaceContentVariant
                }
                Txt {
                    width: parent.width - 24
                    text: qsTr("It asks before it changes a file or runs a command that could change something.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
            }
            Row {
                width: parent.width
                spacing: 8
                Icon {
                    width: 16
                    height: 16
                    path: Icons.terminal
                    color: Theme.surfaceContentVariant
                }
                Txt {
                    width: parent.width - 24
                    text: qsTr("Don't resume it in a terminal while it runs here.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Cancel")
                    onClicked: continueSheet.close()
                }
                Button {
                    text: qsTr("Continue here")
                    onClicked: {
                        Agents.continueSession(Agents.openId)
                        continueSheet.close()
                    }
                }
            }
        }
    }

    // Restarting an outdated host now stops what runs on it.
    Sheet {
        id: restartSheet
        cardWidth: 420
        Column {
            width: parent.width
            spacing: 12
            Txt {
                width: parent.width
                text: qsTr("Restart agents now?")
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                text: page.anyRunning
                    ? qsTr("Threads running now will stop. You can reply to them afterwards to carry on.")
                    : qsTr("The updated version starts in its place.")
                role: "body"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Not now")
                    onClicked: restartSheet.close()
                }
                Button {
                    iconPath: Icons.refresh
                    text: qsTr("Restart")
                    onClicked: {
                        Agents.restartHost()
                        restartSheet.close()
                    }
                }
            }
        }
    }

    // A link in what the agent wrote: where it goes, before it opens.
    Sheet {
        id: linkSheet
        cardWidth: 460
        property string key: ""
        property int index: -1
        property var target: ({})
        function ask(key, index) {
            const json = Agents.linkTarget(key, index)
            if (json.length === 0)
                return
            const target = page.parse(json, ({}))
            if (target.shown === true) {
                Agents.openLink(key, index)
                return
            }
            linkSheet.key = key
            linkSheet.index = index
            linkSheet.target = target
            open()
        }
        Column {
            width: parent.width
            spacing: 12
            Txt {
                width: parent.width
                text: qsTr("Open this link?")
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Rectangle {
                width: parent.width
                height: targetText.height + 20
                radius: Theme.radiusSm
                color: Theme.surfaceContainerHigh
                Txt {
                    id: targetText
                    x: 10
                    y: 10
                    width: parent.width - 20
                    text: linkSheet.target.href || ""
                    role: "code"
                    wrapMode: Text.WrapAnywhere
                    elide: Text.ElideNone
                }
            }
            Txt {
                visible: (linkSheet.target.text || "").length > 0
                width: parent.width
                text: qsTr("Shown as “%1”.").arg(linkSheet.target.text || "")
                role: "bodySmall"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Cancel")
                    onClicked: linkSheet.close()
                }
                Button {
                    iconPath: Icons.openExternal
                    text: qsTr("Open")
                    onClicked: {
                        Agents.openLink(linkSheet.key, linkSheet.index)
                        linkSheet.close()
                    }
                }
            }
        }
    }

    // The dock's "…": always allow (on this PC only) and Stop; for a
    // question, Dismiss and Stop.
    PopupMenu {
        id: dockMenu
        actions: page.questioning
            ? [{ label: qsTr("Dismiss"), icon: Icons.close, run: () => Agents.questionDismiss() },
               { label: qsTr("Stop"), icon: Icons.stopSquare, tint: Theme.tintRed, run: () => Agents.stop() }]
            : dock.overflowActions
        panelWidth: 360
        wrapLabels: true
        z: 20
    }

    // Add image: a file, the clipboard, or a photo from a phone (§V8.4).
    // Lucide's file (ISC, as AgentGitGlyphs.qml credits).
    readonly property string fileIcon: "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7zM14 2v4a2 2 0 0 0 2 2h4"
    PopupMenu {
        id: attachMenu
        actions: [
            { label: qsTr("Choose a file…"), icon: page.fileIcon, run: () => imagePicker.open() },
            { label: qsTr("Paste"), icon: Icons.clipboard, keys: "Ctrl V", run: () => Agents.pasteFromMenu() }
        ].concat((page.composerState.cameras || []).map((phone) => ({
            label: qsTr("Take a photo with %1").arg(phone.name), icon: Icons.camera,
            run: () => Agents.takePhoto(phone.id)
        })))
        panelWidth: 280
        z: 20
    }

    // The open thread's More: continue it on a phone (§V8.5), and its marks.
    PopupMenu {
        id: headMenu
        readonly property var row: page.info
        actions: (page.isThread ? (page.info.handoff || []) : []).map((phone) => ({
            label: qsTr("Continue on %1").arg(phone.name), icon: Icons.phone,
            run: () => Agents.handoff(phone.id)
        })).concat(page.isThread && page.historyOn ? [
            { label: qsTr("Rename"), icon: Icons.edit, keys: "F2", run: () => page.renamingId = Agents.openId },
            { label: qsTr("Snooze"), icon: Icons.moon, enabled: row.archived !== true,
              run: () => page.snooze([Agents.openId], headMenu.anchorX, headMenu.anchorY, page.agentName,
                                     headMenu.row.limitResets || 0) },
            { label: row.archived === true ? qsTr("Unarchive") : qsTr("Archive"), icon: Icons.tray,
              run: () => page.markThreads([Agents.openId], headMenu.row.archived === true ? "unarchive" : "archive") }
        ] : [])
        panelWidth: 260
        z: 20
    }

    // What the phone beside this PC shows of the open thread (§V9.1).
    PopupMenu {
        id: besideMenu
        actions: page.besidePanes.map((word) => ({
            label: page.paneName(word), icon: page.paneIcon(word), checked: page.beside.pane === word,
            run: () => Agents.beside(page.beside.device || "", word)
        }))
        panelWidth: 240
        z: 20
    }

    // The open thread's Run menu (§X5.1).
    AgentRunMenu {
        id: runMenu
        scripts: page.runsState.scripts || []
        z: 20
        onEditRequested: page.editScripts(page.runsState.folder || "")
    }

    // A thread's menu: its marks, rename and delete.
    PopupMenu {
        id: rowMenu
        readonly property var row: page.menuRow
        // A snoozed thread can come back now.
        readonly property bool snoozed: (row.snoozedUntil || 0) > 0
        actions: [
            { label: row.pinned === true ? qsTr("Unpin") : qsTr("Pin"), icon: Icons.pin,
              run: () => page.markThreads([rowMenu.row.id], rowMenu.row.pinned === true ? "unpin" : "pin") },
            { label: row.unread === true ? qsTr("Mark read") : qsTr("Mark unread"), icon: Icons.check,
              run: () => page.markThreads([rowMenu.row.id], rowMenu.row.unread === true ? "read" : "unread") },
            { label: qsTr("Snooze"), icon: Icons.moon, enabled: row.archived !== true,
              run: () => page.snooze([rowMenu.row.id], rowMenu.anchorX, rowMenu.anchorY, rowMenu.row.agent_name || "",
                                     rowMenu.row.limitResets || 0) },
            { label: qsTr("Unsnooze"), icon: Icons.moon, hidden: !snoozed,
              run: () => page.markThreads([rowMenu.row.id], "unsnooze") },
            { label: row.archived === true ? qsTr("Unarchive") : qsTr("Archive"), icon: Icons.tray,
              run: () => page.markThreads([rowMenu.row.id], rowMenu.row.archived === true ? "unarchive" : "archive") },
            { label: qsTr("Rename"), icon: Icons.edit, keys: "F2", run: () => page.renamingId = rowMenu.row.id },
            { label: qsTr("Delete"), icon: Icons.trash, tint: Theme.tintRed,
              enabled: rowMenu.row.state !== "working" && rowMenu.row.state !== "waiting",
              run: () => deleteSheet.ask([rowMenu.row.id]) }
        ].filter((a) => a.hidden !== true)
        panelWidth: 240
        z: 20
    }
    AgentSnoozeMenu {
        id: snoozeMenu
        z: 21
        onSnoozed: (ids, until) => {
            Agents.mark(JSON.stringify(ids), "snooze", until)
            page.picked = []
        }
    }
    // The bulk bar's More.
    PopupMenu {
        id: bulkMenu
        actions: [
            { label: qsTr("Mark unread"), run: () => page.markThreads(page.picked, "unread") },
            { label: qsTr("Mark read"), run: () => page.markThreads(page.picked, "read") },
            // Every thread picked is snoozed: they can all come back now.
            { label: qsTr("Unsnooze"), hidden: !bulkBar.allSnoozed, run: () => {
                page.markThreads(page.picked, "unsnooze")
                page.picked = []
            } }
        ].filter((a) => a.hidden !== true)
        panelWidth: 220
        z: 20
    }
    // The diff's files, as a menu on a narrow page.
    PopupMenu {
        id: diffMenu
        actions: (page.diffState.files || []).map((f, i) => ({
            label: f.path, keys: f.binary ? "" : "+" + f.adds + " −" + f.dels,
            checked: i === (page.diffState.selected || 0), run: () => Agents.diffSelect(i)
        }))
        panelWidth: 360
        z: 20
    }
    AgentRewindDialog {
        id: rewindDialog
        rewind: page.rewindState
    }

    // ---- Source control's menus and dialogs ----
    AgentBranchMenu {
        id: branchMenu
        branches: page.gitState.branches || ({})
        agentName: page.agentName
        locked: page.running ? "running" : ((page.gitState.status || {}).worktree || {}).id !== undefined ? "worktree" : ""
        changed: ((page.gitState.status || {}).files || []).length
        onSwitchAsked: (branch) => {
            Agents.gitBranchMenu(false)
            switchDialog.ask(branch, branchMenu.changed)
        }
    }
    AgentSwitchDialog {
        id: switchDialog
    }
    AgentWorktreeDialog {
        id: worktreeDialog
    }
    AgentMergeDialog {
        id: mergeDialog
        merge: page.gitState.merge || ({})
        pr: (page.gitState.pr || {}).pr || ({})
    }
    AgentPrDialog {
        id: prDialog
        form: page.gitState.form || ({})
        agentName: page.agentName
        defaultBranch: (page.gitState.status || {}).defaultBranch || ""
        pushed: ((page.gitState.status || {}).upstream || "").length > 0 && ((page.gitState.status || {}).ahead || 0) <= 0
        busy: ((page.gitState.job || {}).running === true)
        canAsk: !page.running
        problem: {
            const r = (page.gitState.refused || {}).reason || ""
            return r === "pr_exists" ? qsTr("This branch already has a pull request.")
                : r === "not_pushed" ? qsTr("Push this branch to open a pull request.")
                : r === "not_github" ? qsTr("This folder's remote isn't on GitHub.")
                : ""
        }
    }
    PopupMenu {
        id: gitMenu
        panelWidth: 240
    }

    // A plan's Implement, with the mode it runs in.
    property string planItem: ""
    PopupMenu {
        id: planMenu
        actions: [
            { label: qsTr("Implement, asking before edits and commands"), checked: true,
              run: () => Agents.implement(page.planItem, "supervised") },
            { label: qsTr("Implement, asking before commands"),
              run: () => Agents.implement(page.planItem, "auto_edit") }
        ]
        panelWidth: 360
        wrapLabels: true
        z: 20
    }

    // The open thread's mode (Ctrl+Shift+M), the same three as the model control's.
    PopupMenu {
        id: modeMenu
        readonly property var picker: page.composerState.picker || ({})
        readonly property string mode: picker.mode || "supervised"
        actions: [
            { label: qsTr("Asks before edits and commands"), checked: mode === "supervised",
              run: () => Agents.pick(false, "mode", "supervised") },
            { label: qsTr("Asks before commands"), checked: mode === "auto_edit",
              run: () => Agents.pick(false, "mode", "auto_edit") },
            { label: qsTr("Plan: it changes nothing until you say so"), checked: mode === "plan",
              enabled: picker.plan === true, hidden: picker.plan !== true && picker.agent !== "codex",
              detail: picker.plan === true ? "" : qsTr("Plan isn't available with this version of Codex."),
              run: () => Agents.pick(false, "mode", "plan") }
        ].filter((a) => a.hidden !== true)
        panelWidth: 340
        wrapLabels: true
        z: 20
        onOpenedChanged: if (!opened) composer.focusInput()
    }

    // The model control's panels: the open thread's, and the new-thread form's.
    AgentModelPicker {
        id: threadPicker
        picker: page.composerState.picker || ({})
        pcName: AppController.deviceName
        onOpenedChanged: if (!opened) composer.focusInput()
    }
    AgentModelPicker {
        id: newFormPicker
        newForm: true
        picker: page.newPicker
        pcName: AppController.deviceName
    }

    // Deleting a thread: it can't be brought back.
    Sheet {
        id: deleteSheet
        cardWidth: 420
        // The threads it deletes.
        property var ids: []
        // "Also remove its worktree", for threads that work in one.
        property bool withWorktree: false
        readonly property bool hasWorktree: deleteSheet.ids.some((id) =>
            (((page.shelfRows || page.threads).find((t) => t.id === id) || page.threads.find((t) => t.id === id) || {}).worktree || "").length > 0)
        function ask(ids) {
            deleteSheet.ids = ids
            deleteSheet.withWorktree = false
            open()
        }
        Column {
            width: parent.width
            spacing: 12
            Txt {
                width: parent.width
                text: deleteSheet.ids.length > 1 ? qsTr("Delete %n threads?", "", deleteSheet.ids.length)
                                                 : qsTr("Delete this thread?")
                role: "headline"
                wrapMode: Text.WordWrap
            }
            Txt {
                width: parent.width
                text: qsTr("Its conversation is removed from this PC and your phones. The changes it made to your files stay.")
                role: "body"
                muted: true
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
            Row {
                visible: deleteSheet.hasWorktree
                spacing: 8
                Toggle {
                    anchors.verticalCenter: parent.verticalCenter
                    label: qsTr("Also remove its worktree")
                    checked: deleteSheet.withWorktree
                    onToggled: (on) => deleteSheet.withWorktree = on
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Also remove its worktree")
                    role: "body"
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 10
                Button {
                    variant: "text"
                    text: qsTr("Cancel")
                    onClicked: deleteSheet.close()
                }
                Button {
                    iconPath: Icons.trash
                    text: qsTr("Delete")
                    onClicked: {
                        for (const id of deleteSheet.ids)
                            Agents.deleteThreadWith(id, deleteSheet.hasWorktree && deleteSheet.withWorktree)
                        page.picked = page.picked.filter((x) => deleteSheet.ids.indexOf(x) < 0)
                        deleteSheet.close()
                    }
                }
            }
        }
    }
}
