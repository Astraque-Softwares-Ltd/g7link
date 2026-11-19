// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Dialogs
import app.g7link

// Agents: Claude and Codex threads on this PC, and the sessions started here
// outside G7, on the left (needs you, working, recent, on this PC); on the
// right the open one (its transcript, what it asks, the reply box or Continue
// here, and its details), a new thread, or the setup. It doesn't need a
// paired phone.
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
    readonly property var asks: parse(Agents.asks, [])
    readonly property var steps: parse(Agents.steps, [])
    readonly property var changes: parse(Agents.changes, [])

    readonly property bool connected: Agents.status === "connected"
    readonly property var readyAgents: connected
        ? (setup.agents || []).filter((a) => a.installed && a.signedIn && a.problem.length === 0) : []
    readonly property var usableFolders: connected ? (setup.folders || []).filter((f) => f.exists) : []
    readonly property bool ready: readyAgents.length > 0 && usableFolders.length > 0
    readonly property string openKind: Agents.openKind
    readonly property bool isThread: openKind === "thread"
    readonly property bool isSession: openKind === "session"
    readonly property string shown: openKind.length > 0 ? openKind
        : (pane === "setup" || (pane !== "new" && !ready) ? "setup" : "new")
    readonly property bool narrow: width < 720
    // On a narrow page the list fills it until something is picked.
    readonly property bool listOnly: narrow && openKind.length === 0 && pane.length === 0
    // The details sit beside the transcript only while it keeps 480 px
    // (from about 1230 px of window); otherwise they fold into Details.
    readonly property bool inspectorInline: width - 300 - 250 >= 480

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
    readonly property var needsRows: threads.filter((t) => t.state === "waiting" && matches(t))
    readonly property var workingRows: threads.filter((t) => t.state === "working" && matches(t))
    readonly property var recentRows: threads.filter((t) => t.state !== "waiting" && t.state !== "working" && matches(t))
    readonly property var sessionRows: sessions.filter((s) => matches(s))
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
    readonly property string stateWord: {
        if (isSession) {
            switch (phase) {
            case "needs_you": return info.guess ? qsTr("May need you") : qsTr("Needs you")
            case "working": return qsTr("Working")
            case "idle": return qsTr("Idle")
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
    readonly property color stateColor: {
        switch (phase) {
        case "waiting":
        case "needs_you": return Theme.tintAmber
        case "working": return Theme.tintBlue
        case "done":
        case "idle": return Theme.tintTeal
        case "failed": return Theme.error
        default: return Theme.outline
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
        Agents.close()
        page.pane = "new"
        Qt.callLater(() => taskInput.forceActiveFocus())
    }
    function openSetup() {
        Agents.close()
        page.pane = "setup"
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
        Agents.close()
        page.pane = ""
    }
    function addFolder(sessionId) {
        page.continueAfterAdd = sessionId
        folderPicker.open()
    }

    onActiveChanged: Agents.pageVisible(active)
    Component.onCompleted: Agents.pageVisible(active)
    Component.onDestruction: Agents.pageVisible(false)

    Timer {
        interval: page.anyRunning || page.running ? 1000 : 30000
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
        if (isThread && !running)
            return { label: qsTr("Send"), keys: "Enter", run: () => composer.send() }
        if (isSession && info.can_continue === true)
            return { label: qsTr("Continue here"), keys: "Ctrl Enter", run: () => continueSheet.open() }
        return { label: qsTr("New thread"), keys: "Ctrl N", run: () => page.startNew() }
    }
    readonly property var screenActions: [
        { label: qsTr("New thread"), icon: Icons.plus, tint: Theme.accent, keys: "Ctrl N", run: () => page.startNew() },
        { label: qsTr("Search threads"), icon: Icons.search, keys: "Ctrl F", run: () => search.input.forceActiveFocus() },
        { label: qsTr("Stop"), icon: Icons.stopSquare, tint: Theme.tintRed, enabled: page.running,
          run: () => Agents.stop() },
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
        { label: qsTr("Details"), icon: Icons.info, checked: page.detailsOpen,
          enabled: (page.isThread || page.isSession) && !page.inspectorInline,
          run: () => { page.detailsOpen = !page.detailsOpen } },
        { label: qsTr("Delete thread"), icon: Icons.trash, tint: Theme.tintRed, enabled: page.isThread && !page.running,
          run: () => deleteSheet.open() },
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
    Shortcut {
        sequence: "Esc"
        enabled: page.active && (page.openKind.length > 0 || page.pane.length > 0)
                 && !continueSheet.opened && !folderSheet.opened && !deleteSheet.opened
                 && !AppController.commandPaletteOpen
        onActivated: page.back()
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

    // A list section: its title and count, then its rows.
    component ListSection: Column {
        id: section
        property string title
        property var rows: []
        property bool sessions: false
        property bool counted: true
        property real now: Date.now()
        signal picked(string id)
        visible: rows.length > 0
        topPadding: 6
        Item {
            width: section.width
            height: 28
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
                text: String(section.rows.length)
                role: "bodySmall"
                muted: true
            }
        }
        Repeater {
            model: section.rows
            delegate: AgentRow {
                id: listRow
                required property var modelData
                width: section.width
                item: modelData
                session: section.sessions
                now: section.now
                selected: Agents.openKind === (section.sessions ? "session" : "thread") && Agents.openId === modelData.id
                onClicked: section.picked(listRow.modelData.id)
            }
        }
    }

    // ---- The list ----
    Item {
        id: listPane
        visible: !page.narrow || page.listOnly
        width: page.narrow ? page.width : 300
        height: page.height
        clip: true

        Column {
            id: listTop
            x: 8
            y: 8
            width: parent.width - 16
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
            }
        }

        Flickable {
            id: listFlick
            x: 8
            width: parent.width - 16
            anchors.top: listTop.bottom
            anchors.topMargin: 4
            anchors.bottom: setupRow.top
            anchors.bottomMargin: 4
            contentHeight: listColumn.height
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            Column {
                id: listColumn
                width: listFlick.width
                bottomPadding: 8
                ListSection {
                    width: parent.width
                    title: qsTr("Needs you")
                    rows: page.needsRows
                    now: page.now
                    onPicked: (id) => page.openRow("thread", id)
                }
                ListSection {
                    width: parent.width
                    title: qsTr("Working")
                    rows: page.workingRows
                    now: page.now
                    onPicked: (id) => page.openRow("thread", id)
                }
                ListSection {
                    width: parent.width
                    title: qsTr("Recent")
                    rows: page.recentRows
                    counted: false
                    now: page.now
                    onPicked: (id) => page.openRow("thread", id)
                }
                ListSection {
                    width: parent.width
                    title: qsTr("On this PC")
                    rows: page.sessionRows
                    sessions: true
                    counted: false
                    now: page.now
                    onPicked: (id) => page.openRow("session", id)
                }
                Txt {
                    visible: page.threads.length === 0 && page.sessions.length === 0 && page.connected
                    x: Theme.rowPadding
                    width: parent.width - 2 * Theme.rowPadding
                    topPadding: 12
                    text: qsTr("No threads yet. Start one with New thread.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
                Txt {
                    visible: page.query.length > 0 && page.needsRows.length + page.workingRows.length
                             + page.recentRows.length + page.sessionRows.length === 0
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

        ActionRow {
            id: setupRow
            x: 8
            width: parent.width - 16
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 8
            iconPath: Icons.settings
            tint: Theme.surfaceContentVariant
            title: qsTr("Setup")
            meta: page.connected ? page.setupSummary : ""
            selected: page.shown === "setup" && !page.listOnly
            onClicked: page.openSetup()
        }

        Rectangle {
            visible: !page.narrow
            anchors.right: parent.right
            width: 1
            height: parent.height
            color: Theme.line
        }
    }

    // ---- The right side ----
    Item {
        id: mainPane
        visible: !page.listOnly
        x: page.narrow ? 0 : listPane.width
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
                visible: page.narrow
                x: 8
                anchors.verticalCenter: parent.verticalCenter
                iconPath: Icons.back
                label: qsTr("Back to the list")
                onClicked: page.back()
            }
            Column {
                anchors.left: backButton.visible ? backButton.right : parent.left
                anchors.leftMargin: backButton.visible ? 8 : 16
                anchors.right: headButtons.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                spacing: 1
                Txt {
                    width: parent.width
                    text: page.shown === "setup" ? qsTr("Setup")
                        : page.shown === "new" ? qsTr("New thread") : (page.info.title || "")
                    role: "title"
                    maximumLineCount: 1
                }
                Row {
                    width: parent.width
                    spacing: 6
                    visible: page.isThread || page.isSession
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 7
                        height: 7
                        radius: 3.5
                        readonly property bool ring: page.isSession && page.info.guess === true && page.phase === "needs_you"
                        color: ring ? "transparent" : page.stateColor
                        border.width: ring ? 1.5 : 0
                        border.color: page.stateColor
                    }
                    Txt {
                        width: Math.min(implicitWidth, parent.width - 13)
                        anchors.verticalCenter: parent.verticalCenter
                        text: [page.stateWord, page.agentName, page.isSession ? page.info.shown_cwd : page.info.folder, page.originText]
                            .filter((s) => (s || "").length > 0).join(" · ")
                        role: "bodySmall"
                        muted: true
                        maximumLineCount: 1
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
                Button {
                    visible: (page.isThread || page.isSession) && !page.inspectorInline
                    variant: page.detailsOpen ? "tonal" : "text"
                    size: "sm"
                    iconPath: Icons.clipboardList
                    text: qsTr("Details")
                    onClicked: page.detailsOpen = !page.detailsOpen
                }
                Button {
                    visible: page.running
                    variant: "tonal"
                    size: "sm"
                    iconPath: Icons.stopSquare
                    text: qsTr("Stop")
                    busy: Agents.busy
                    onClicked: Agents.stop()
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
            visible: page.isThread || page.isSession
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            anchors.left: parent.left
            anchors.right: page.inspectorInline ? inspector.left : parent.right
            onVisibleChanged: transcript.follow = true

            ListView {
                id: transcript
                // Keeps the newest in view until the user scrolls up.
                property bool follow: true
                anchors.top: parent.top
                anchors.bottom: bottomBar.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: 16
                anchors.rightMargin: 16
                model: openView.visible ? AgentItems : null
                clip: true
                spacing: 2
                boundsBehavior: Flickable.StopAtBounds
                delegate: AgentItem {}
                onMovementEnded: follow = atYEnd
                onAtYEndChanged: if (atYEnd) follow = true
                // Scrolling up to read stops following the end.
                WheelHandler {
                    target: null
                    blocking: false
                    onWheel: (event) => {
                        if (event.angleDelta.y > 0 || event.pixelDelta.y > 0)
                            transcript.follow = false
                    }
                }
                onContentHeightChanged: if (follow) Qt.callLater(() => transcript.positionViewAtEnd())
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
                    Spinner {
                        visible: Agents.loading
                        width: 18
                        height: 18
                    }
                }

                footer: Column {
                    width: transcript.width
                    topPadding: 8
                    bottomPadding: 12
                    spacing: 10

                    // What it is doing now.
                    Rectangle {
                        visible: page.running && (page.info.step || "").length > 0
                        width: parent.width
                        height: visible ? nowColumn.height + 20 : 0
                        radius: Theme.radiusMd
                        color: Theme.surfaceContainerLow
                        border.width: 1
                        border.color: Theme.outlineVariant
                        Column {
                            id: nowColumn
                            x: 12
                            y: 10
                            width: parent.width - 24
                            spacing: 8
                            Item {
                                width: parent.width
                                height: 22
                                Spinner {
                                    id: nowSpinner
                                    anchors.verticalCenter: parent.verticalCenter
                                    width: 14
                                    height: 14
                                }
                                Txt {
                                    anchors.left: nowSpinner.right
                                    anchors.leftMargin: 8
                                    anchors.right: nowTime.left
                                    anchors.rightMargin: 8
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: qsTr("Now: %1").arg(page.info.step || "")
                                    weight: 500
                                    maximumLineCount: 1
                                }
                                Txt {
                                    id: nowTime
                                    anchors.right: parent.right
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: page.elapsed(page.since)
                                    role: "bodySmall"
                                    muted: true
                                }
                            }
                            Rectangle {
                                visible: !!page.runningStep && (page.runningStep.detail || "").length > 0
                                width: parent.width
                                height: visible ? stepDetail.height + 30 : 0
                                radius: Theme.radiusSm
                                color: Theme.surfaceContainerHigh
                                Row {
                                    x: 10
                                    y: 6
                                    spacing: 6
                                    Icon {
                                        width: 13
                                        height: 13
                                        path: Icons.folder
                                        color: Theme.surfaceContentVariant
                                    }
                                    Txt {
                                        text: page.info.folder || ""
                                        role: "caption"
                                        muted: true
                                    }
                                }
                                Txt {
                                    id: stepDetail
                                    x: 10
                                    y: 24
                                    width: parent.width - 20
                                    text: page.runningStep ? (page.runningStep.detail || "") : ""
                                    role: "code"
                                    wrapMode: Text.WrapAnywhere
                                    elide: Text.ElideNone
                                    maximumLineCount: 6
                                }
                            }
                        }
                    }

                    // What the thread asks.
                    Repeater {
                        model: page.isThread ? page.asks : []
                        delegate: Rectangle {
                            id: askCard
                            required property var modelData
                            readonly property var diffLines: (modelData.diff || "").split("\n").slice(0, 200)
                            width: transcript.width
                            height: askColumn.height + 24
                            radius: Theme.radiusMd
                            color: Theme.surfaceContainerLow
                            border.width: 1
                            border.color: Theme.tintAmber
                            Column {
                                id: askColumn
                                x: 12
                                y: 12
                                width: parent.width - 24
                                spacing: 10
                                Txt {
                                    width: parent.width
                                    text: askCard.modelData.kind === "command"
                                        ? qsTr("%1 wants to run a command").arg(page.agentName)
                                        : askCard.modelData.kind === "edit"
                                            ? qsTr("%1 wants to change %2").arg(page.agentName).arg(askCard.modelData.path || askCard.modelData.title)
                                            : askCard.modelData.title
                                    weight: 500
                                    wrapMode: Text.WordWrap
                                    elide: Text.ElideNone
                                }
                                Rectangle {
                                    visible: (askCard.modelData.command || "").length > 0
                                    width: parent.width
                                    height: visible ? askCommand.height + 16 : 0
                                    radius: Theme.radiusSm
                                    color: Theme.surfaceContainerHigh
                                    Txt {
                                        id: askCommand
                                        x: 10
                                        y: 8
                                        width: parent.width - 20
                                        text: askCard.modelData.command || ""
                                        role: "code"
                                        wrapMode: Text.WrapAnywhere
                                        elide: Text.ElideNone
                                    }
                                }
                                Rectangle {
                                    visible: (askCard.modelData.diff || "").length > 0
                                    width: parent.width
                                    height: visible ? askDiff.height + 12 : 0
                                    radius: Theme.radiusSm
                                    color: Theme.surfaceContainerHigh
                                    clip: true
                                    Column {
                                        id: askDiff
                                        y: 6
                                        width: parent.width
                                        Repeater {
                                            model: askCard.diffLines
                                            delegate: Rectangle {
                                                id: askLine
                                                required property string modelData
                                                width: askDiff.width
                                                height: diffLine.height
                                                color: modelData.startsWith("+") ? Theme.wash(Theme.tintGreen)
                                                    : (modelData.startsWith("-") ? Theme.wash(Theme.error) : "transparent")
                                                Txt {
                                                    id: diffLine
                                                    x: 10
                                                    width: parent.width - 20
                                                    text: askLine.modelData.length > 0 ? askLine.modelData : " "
                                                    role: "code"
                                                    wrapMode: Text.WrapAnywhere
                                                    elide: Text.ElideNone
                                                }
                                            }
                                        }
                                    }
                                }
                                Txt {
                                    visible: (askCard.modelData.reason || "").length > 0
                                    width: parent.width
                                    text: askCard.modelData.reason || ""
                                    role: "bodySmall"
                                    muted: true
                                    wrapMode: Text.WordWrap
                                    elide: Text.ElideNone
                                }
                                Txt {
                                    visible: askCard.modelData.truncated === true
                                    width: parent.width
                                    text: qsTr("It's too long to show in full here.")
                                    role: "bodySmall"
                                    muted: true
                                    wrapMode: Text.WordWrap
                                }
                                Flow {
                                    width: parent.width
                                    spacing: 8
                                    layoutDirection: Qt.RightToLeft
                                    Button {
                                        text: qsTr("Allow")
                                        busy: Agents.busy
                                        onClicked: Agents.answer(askCard.modelData.token, "allow")
                                    }
                                    Button {
                                        visible: askCard.modelData.can_allow_thread === true
                                        variant: "tonal"
                                        text: qsTr("Allow for this thread")
                                        enabled: !Agents.busy
                                        onClicked: Agents.answer(askCard.modelData.token, "allow_thread")
                                    }
                                    Button {
                                        variant: "text"
                                        text: qsTr("Deny")
                                        enabled: !Agents.busy
                                        onClicked: Agents.answer(askCard.modelData.token, "deny")
                                    }
                                }
                                Txt {
                                    width: parent.width
                                    text: qsTr("If you deny, the agent carries on another way.")
                                    role: "caption"
                                    muted: true
                                    horizontalAlignment: Text.AlignRight
                                    wrapMode: Text.WordWrap
                                }
                            }
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
                        border.color: page.info.guess ? Theme.outlineVariant : Theme.tintAmber
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
                TouchpadGlide { view: transcript }
            }

            // ---- Under the transcript: the reply box, or Continue here ----
            Item {
                id: bottomBar
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                height: page.isThread ? composer.height + 20 : sessionBar.height + 16

                Item {
                    id: composer
                    visible: page.isThread
                    x: 16
                    y: 6
                    width: parent.width - 32
                    height: Math.min(Math.max(input.contentHeight + 18, 38), 150)
                    readonly property bool canSend: !page.running && input.text.trim().length > 0 && !Agents.busy
                    function send() {
                        if (!composer.canSend)
                            return
                        Agents.send(input.text)
                        input.text = ""
                    }
                    Rectangle {
                        anchors.left: parent.left
                        anchors.right: sendButton.left
                        anchors.rightMargin: 8
                        height: parent.height
                        radius: Theme.rowRadius
                        color: Theme.raised
                        border.width: input.activeFocus ? 1 : 0
                        border.color: Theme.focusRing
                        opacity: page.running ? 0.7 : 1
                        Flickable {
                            id: inputScroll
                            anchors.fill: parent
                            anchors.leftMargin: 12
                            anchors.rightMargin: 12
                            anchors.topMargin: 9
                            anchors.bottomMargin: 9
                            contentHeight: input.contentHeight
                            clip: true
                            boundsBehavior: Flickable.StopAtBounds
                            TextEdit {
                                id: input
                                width: inputScroll.width
                                wrapMode: TextEdit.Wrap
                                readOnly: page.running
                                textFormat: TextEdit.PlainText
                                font.family: Theme.fontUi
                                font.pixelSize: Theme.typography.body.size
                                color: Theme.surfaceContent
                                selectionColor: Theme.selection
                                selectedTextColor: Theme.surfaceContent
                                Accessible.name: qsTr("Message %1").arg(page.agentName)
                                // Enter sends; Shift+Enter starts a new line.
                                Keys.onPressed: (event) => {
                                    if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                                        && !(event.modifiers & Qt.ShiftModifier)) {
                                        composer.send()
                                        event.accepted = true
                                    }
                                }
                                onCursorRectangleChanged: {
                                    if (cursorRectangle.y < inputScroll.contentY)
                                        inputScroll.contentY = cursorRectangle.y
                                    else if (cursorRectangle.y + cursorRectangle.height > inputScroll.contentY + inputScroll.height)
                                        inputScroll.contentY = cursorRectangle.y + cursorRectangle.height - inputScroll.height
                                }
                                Txt {
                                    visible: input.text.length === 0
                                    width: input.width
                                    text: page.running ? qsTr("You can reply when %1 stops.").arg(page.agentName)
                                                       : qsTr("Message %1").arg(page.agentName)
                                    muted: true
                                }
                            }
                        }
                    }
                    IconButton {
                        id: sendButton
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: 4
                        iconPath: Icons.send
                        label: qsTr("Send")
                        enabled: composer.canSend
                        onClicked: composer.send()
                    }
                }

                Column {
                    id: sessionBar
                    visible: page.isSession
                    x: 16
                    y: 6
                    width: parent.width - 32
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
            visible: (page.isThread || page.isSession) && (page.inspectorInline || page.detailsOpen)
            anchors.top: headLine.bottom
            anchors.bottom: parent.bottom
            anchors.right: parent.right
            width: page.inspectorInline ? 250 : Math.min(280, mainPane.width - 40)
            color: Theme.surface
            z: 2
            Rectangle {
                width: 1
                height: parent.height
                color: Theme.line
            }
            Flickable {
                id: inspectorFlick
                anchors.fill: parent
                anchors.leftMargin: 1
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
                                    color: Theme.tintGreen
                                }
                                Rectangle {
                                    visible: planStep.modelData.status !== "completed"
                                    x: Theme.rowPadding + 3
                                    y: 8
                                    width: 9
                                    height: 9
                                    radius: 4.5
                                    color: planStep.modelData.status === "in_progress" ? Theme.accent : "transparent"
                                    border.width: planStep.modelData.status === "in_progress" ? 0 : 1.2
                                    border.color: Theme.outline
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
                                        color: Theme.tintGreen
                                    }
                                    Txt {
                                        text: "−" + changeRow.modelData.removed
                                        role: "bodySmall"
                                        color: Theme.error
                                    }
                                }
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
                                                                       : qsTr("Asks before edits and commands")) },
                                { icon: Icons.history, text: page.clockText }
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
            function start() {
                const task = taskInput.text.trim()
                if (task.length === 0 || newView.agent.length === 0 || newView.folder.length === 0)
                    return
                Agents.start(newView.agent, newView.folder, task, newView.mode)
                taskInput.text = ""
            }

            Column {
                id: form
                x: 16
                y: 16
                width: Math.min(720, newView.width - 32)
                spacing: 16

                Rectangle {
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
                        ]
                        value: newView.mode
                        onPicked: (value) => { newView.mode = value }
                    }
                    Txt {
                        width: parent.width
                        text: newView.mode === "auto_edit"
                            ? qsTr("It changes files without asking, and asks before a command that could change something.")
                            : qsTr("It asks before it changes a file or runs a command that could change something.")
                        role: "bodySmall"
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
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
                        busy: Agents.busy
                        enabled: page.ready && taskInput.text.trim().length > 0
                        onClicked: newView.start()
                    }
                }
            }
            TouchpadGlide { view: newView }
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
                onAddFolderRequested: page.addFolder("")
            }
            TouchpadGlide { view: setupView }
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

    // Deleting a thread: it can't be brought back.
    Sheet {
        id: deleteSheet
        cardWidth: 420
        Column {
            width: parent.width
            spacing: 12
            Txt {
                width: parent.width
                text: qsTr("Delete this thread?")
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
                        Agents.deleteThread(Agents.openId)
                        deleteSheet.close()
                    }
                }
            }
        }
    }
}
