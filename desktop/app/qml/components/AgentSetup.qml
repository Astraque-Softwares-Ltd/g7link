// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import app.g7link

// The Agents page's setup: the agent service, the agents and how they're
// signed in, the folders they may work in, each phone's three switches, and
// following sessions started outside G7. No row has a free-width slot: text
// takes the room left by fixed controls, paths are one line cut in the
// middle (the whole path shows on hover), and commands wrap anywhere.
Column {
    id: area
    // Agents.setup, parsed.
    property var setup: ({})
    // Agents.gitSetup: { available, gh: { gh, version, hosts }, worktreesRead, worktrees: [...] }
    property var gitSetup: ({})
    signal addFolderRequested
    // A worktree's Remove: its id, branch and commits on no remote.
    signal removeWorktree(string id, string branch, int unpushed)
    // "Bring in earlier sessions".
    signal importRequested

    readonly property bool connected: setup.status === "connected"
    readonly property var agents: connected ? (setup.agents || []) : []
    readonly property var folders: connected ? (setup.folders || []) : []
    // The host has phase 2's settings (keeping awake, MCP per folder).
    readonly property bool control: connected && setup.control === true
    // The host keeps a store of threads, and restore points.
    readonly property bool history: connected && setup.history === true
    readonly property bool undo: connected && setup.undo === true
    // The host keeps each folder's scripts (phase 6).
    readonly property bool scriptsOn: connected && setup.scripts === true
    // Where a folder's card starts, for "Edit scripts…" (-1: not listed).
    function folderY(id) {
        const at = area.folders.findIndex((f) => f.id === id)
        const item = at >= 0 ? folderRepeater.itemAt(at) : null
        return item ? item.mapToItem(area, 0, 0).y : -1
    }
    readonly property var store: setup.store || ({})

    // "61 MB", "820 KB": what the host keeps on disk.
    function size(bytes) {
        if (bytes >= 1073741824)
            return qsTr("%1 GB").arg((bytes / 1073741824).toLocaleString(Qt.locale(), "f", 1))
        if (bytes >= 1048576)
            return qsTr("%1 MB").arg(Math.round(bytes / 1048576))
        return qsTr("%1 KB").arg(Math.max(1, Math.round(bytes / 1024)))
    }
    // Codex follows a folder's switch too, or always loads its servers.
    readonly property bool codexSwitch: agents.some((a) => a.id === "codex" && a.mcpSwitch === true)
    readonly property bool codexShown: agents.some((a) => a.id === "codex" && a.installed)
    // Tabler's plug (MIT, as Icons.qml credits).
    readonly property string plugIcon: "M9.8 6l8.2 8.2-2 2.1a5.8 5.8 0 1 1-8.3-8.3zM4 20l3.5-3.5M15 4l-3.5 3.5M20 9l-3.5 3.5"

    spacing: 18

    // The line under an agent's name: its command and version, then how it's signed in.
    function agentDetail(a) {
        if (!a.installed)
            return qsTr("%1 isn't installed on this PC.").arg(a.name)
        const version = a.version.length > 0 ? a.id + " " + a.version : a.id
        if (a.problem.length > 0)
            return a.problem
        if (!a.signedIn)
            return version
        if (a.auth === "api_key")
            return version + " · " + (a.id === "claude"
                ? qsTr("using ANTHROPIC_API_KEY, billed to your API account")
                : qsTr("using an API key, billed to your API account"))
        if (a.plan.length > 0)
            return version + " · " + qsTr("signed in with %1").arg(a.plan.charAt(0).toUpperCase() + a.plan.slice(1))
        return version + " · " + qsTr("signed in")
    }
    function agentState(a) {
        if (!a.installed)
            return qsTr("Not installed")
        if (a.problem.length > 0)
            return qsTr("Needs an update")
        return a.signedIn ? qsTr("Ready") : qsTr("Signed out")
    }
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
    // When a window resets: the time today, else the day and the time.
    function resetsAt(ms) {
        const when = new Date(ms)
        const time = when.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
        return ms - Date.now() < 86400000 ? time : when.toLocaleDateString(Qt.locale(), "ddd") + " " + time
    }
    // "5-hour window: 41% used, resets 18:00 · weekly window: …", as the CLI reported them.
    function limitsLine(a) {
        return (a.limits || []).filter((l) => l.usedPct >= 0).map((l) => l.resetsMs > 0
            ? qsTr("%1 window: %2% used, resets %3").arg(windowName(l.window)).arg(l.usedPct).arg(resetsAt(l.resetsMs))
            : qsTr("%1 window: %2% used").arg(windowName(l.window)).arg(l.usedPct)).join(" · ")
    }
    // The servers the agents last reported in a folder, by name.
    function mcpLine(f) {
        const names = []
        for (const seen of (f.mcpSeen || [])) {
            for (const n of seen.names) {
                if (names.indexOf(n) < 0)
                    names.push(n)
            }
        }
        if (names.length === 0)
            return qsTr("No thread has run here yet; their names show after one does.")
        return qsTr("%n MCP server(s): %1.", "", names.length).arg(names.join(", "))
    }
    function toggleOn(deviceId, name) {
        const t = AppController.togglesRevision >= 0
            ? AppController.deviceToggles(deviceId).find((x) => x.name === name) : undefined
        return Boolean(t && t.on)
    }

    // A row: an icon, text that takes the room left, and fixed controls.
    component Line: Item {
        id: line
        property string iconPath
        property color iconColor: Theme.surfaceContentVariant
        property string title
        property string description
        // The title is a path or a command: one line, cut in the middle.
        property bool mono: false
        // Shown on hover when the title is cut.
        property string fullText
        property real indent: 0
        default property alias trailing: trailingRow.data

        width: parent ? parent.width : 0
        implicitHeight: Math.max(Theme.rowHeight, layout.implicitHeight + 14)

        RowLayout {
            id: layout
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: Theme.rowPadding + line.indent
            anchors.rightMargin: Theme.rowPadding
            anchors.verticalCenter: parent.verticalCenter
            spacing: 12
            Icon {
                visible: line.iconPath.length > 0
                Layout.alignment: Qt.AlignTop
                Layout.topMargin: 1
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                path: line.iconPath
                color: line.iconColor
            }
            Column {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                spacing: 2
                Txt {
                    id: titleText
                    width: parent.width
                    text: line.title
                    role: line.mono ? "code" : "body"
                    wrapMode: line.mono ? Text.NoWrap : Text.WordWrap
                    elide: line.mono ? Text.ElideMiddle : Text.ElideNone
                    maximumLineCount: line.mono ? 1 : 4
                    HoverHandler { id: titleHover }
                }
                Txt {
                    visible: line.description.length > 0
                    width: parent.width
                    text: line.description
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
            }
            Row {
                id: trailingRow
                Layout.alignment: Qt.AlignVCenter
                spacing: 8
            }
        }

        // The whole path, over the window.
        Rectangle {
            parent: line.Window.contentItem ? line.Window.contentItem : line
            z: 1000
            visible: line.fullText.length > 0 && titleHover.hovered && titleText.truncated
            readonly property point at: visible ? line.mapToItem(parent, layout.x + titleText.x + 30, line.height) : Qt.point(0, 0)
            x: Math.max(8, Math.min(at.x, (parent ? parent.width : 0) - width - 8))
            y: at.y
            width: Math.min(tipText.implicitWidth + 18, 520)
            height: tipText.implicitHeight + 10
            radius: Theme.radiusSm
            color: Theme.surfaceContainerHighest
            border.width: 1
            border.color: Theme.outlineVariant
            Txt {
                id: tipText
                anchors.centerIn: parent
                width: Math.min(implicitWidth, 502)
                text: line.fullText
                role: "caption"
                wrapMode: Text.WrapAnywhere
                elide: Text.ElideNone
            }
        }
    }

    // ---- The agent service, when it isn't reached ----
    Line {
        visible: !area.connected
        iconPath: area.setup.status === "failed" ? Icons.warning : Icons.info
        title: area.setup.status === "not_installed"
            ? qsTr("The agent service isn't installed with this copy of G7 Link.")
            : area.setup.status === "connecting"
                ? qsTr("Starting the agent service…")
                : area.setup.status === "failed"
                    ? qsTr("Couldn't reach the agent service.")
                    : qsTr("The agent service isn't running.")
        Button {
            visible: area.setup.status !== "not_installed" && area.setup.status !== "connecting"
            size: "sm"
            variant: "tonal"
            text: area.setup.status === "failed" ? qsTr("Try again") : qsTr("Start it")
            onClicked: Agents.startHost()
        }
    }

    // ---- Agents ----
    Column {
        visible: area.connected
        width: parent.width
        SectionLabel { width: parent.width; text: qsTr("Agents") }
        Repeater {
            model: area.agents
            delegate: Line {
                id: agentLine
                required property var modelData
                readonly property bool needsLogin: modelData.installed && !modelData.signedIn && modelData.problem.length === 0
                iconPath: Icons.terminal
                iconColor: modelData.installed && modelData.signedIn && modelData.problem.length === 0
                    ? Theme.tintTeal : Theme.surfaceContentVariant
                title: modelData.name
                description: {
                    const detail = needsLogin
                        ? qsTr("Sign in by running %1 in a terminal.").arg(modelData.loginHint)
                        : area.agentDetail(modelData)
                    const limits = area.limitsLine(modelData)
                    return limits.length > 0 ? detail + "\n" + limits : detail
                }
                Button {
                    visible: agentLine.needsLogin
                    variant: "text"
                    size: "sm"
                    iconPath: Icons.copy
                    text: qsTr("Copy")
                    onClicked: Agents.copy(agentLine.modelData.loginHint)
                }
                Txt {
                    width: 96
                    horizontalAlignment: Text.AlignRight
                    text: area.agentState(agentLine.modelData)
                    role: "bodySmall"
                    muted: true
                }
            }
        }
    }

    // ---- While an agent works ----
    Column {
        visible: area.control
        width: parent.width
        Line {
            iconPath: Icons.bolt
            title: qsTr("Keep this PC awake while an agent works")
            Toggle {
                enabled: area.setup.keepAwake === true || area.setup.keepAwake === false
                label: qsTr("Keep this PC awake while an agent works")
                checked: area.setup.keepAwake !== false
                onToggled: (on) => Agents.setKeepAwake(on)
            }
        }
        // The host posts it at the reset time; it opens the thread, nothing resumes.
        // An older host has no such alarm.
        Line {
            visible: area.setup.limitAlarm === true || area.setup.limitAlarm === false
            iconPath: Icons.bell
            title: qsTr("Tell me when a limit resets")
            Toggle {
                label: qsTr("Tell me when a limit resets")
                checked: area.setup.limitAlarm !== false
                onToggled: (on) => Agents.setLimitAlarm(on)
            }
        }
        Line {
            iconPath: Icons.send
            title: qsTr("While it works, a new message")
            description: area.setup.followUp === "queue"
                ? qsTr("Enter queues it for when the turn ends; Alt+Enter sends it into the turn.")
                : qsTr("Enter sends it into the turn; Alt+Enter queues it for when the turn ends.")
            Segmented {
                options: [
                    { value: "steer", label: qsTr("Steers it") },
                    { value: "queue", label: qsTr("Waits its turn") }
                ]
                value: area.setup.followUp || "steer"
                onPicked: (value) => Agents.setFollowUp(value)
            }
        }
    }

    // ---- Folders ----
    // What G7 keeps: threads, and the restore points' size; earlier
    // sessions can be brought in.
    Column {
        visible: area.history
        width: parent.width
        SectionLabel { width: parent.width; text: qsTr("History") }
        Line {
            iconPath: Icons.history
            title: area.store.threads === undefined ? qsTr("Threads in G7")
                : area.undo ? qsTr("%1 threads · %2 · restore points %3").arg(area.store.threads)
                              .arg(area.size(area.store.bytes || 0)).arg(area.size(area.store.checkpointBytes || 0))
                : qsTr("%1 threads · %2").arg(area.store.threads).arg(area.size(area.store.bytes || 0))
            Button {
                variant: "tonal"
                size: "sm"
                iconPath: Icons.download
                text: qsTr("Bring in earlier sessions")
                onClicked: area.importRequested()
            }
        }
    }

    Column {
        visible: area.connected
        width: parent.width
        Item {
            width: parent.width
            height: 34
            SectionLabel {
                anchors.left: parent.left
                anchors.right: addButton.left
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Folders agents may work in")
            }
            Button {
                id: addButton
                anchors.right: parent.right
                anchors.rightMargin: Theme.rowPadding
                anchors.verticalCenter: parent.verticalCenter
                variant: "tonal"
                size: "sm"
                iconPath: Icons.plus
                text: qsTr("Add folder…")
                onClicked: area.addFolderRequested()
            }
        }
        Line {
            visible: area.folders.length === 0
            iconPath: Icons.folder
            title: qsTr("No folders yet.")
        }
        Repeater {
            id: folderRepeater
            model: area.folders
            delegate: Column {
                id: folderItem
                required property var modelData
                width: parent.width
                Line {
                    iconPath: Icons.folder
                    iconColor: folderItem.modelData.exists ? Theme.tintAmber : Theme.surfaceContentVariant
                    title: folderItem.modelData.shownPath
                    fullText: folderItem.modelData.path
                    mono: true
                    description: folderItem.modelData.exists ? "" : qsTr("Not found")
                    Accessible.name: folderItem.modelData.path
                    Button {
                        variant: "text"
                        size: "sm"
                        iconPath: Icons.trash
                        text: qsTr("Remove")
                        onClicked: Agents.removeFolder(folderItem.modelData.id)
                    }
                }
                // The user's own MCP servers here: on unless turned off.
                Line {
                    visible: area.control
                    indent: 30
                    iconPath: area.plugIcon
                    title: qsTr("Use my MCP servers here")
                    description: {
                        const lines = [area.codexSwitch
                            ? qsTr("Claude and Codex load the MCP servers you set up for them. Each tool still asks first.")
                            : qsTr("Claude loads the MCP servers you set up for it. Each tool still asks first.")]
                        if (!area.codexSwitch && area.codexShown)
                            lines.push(qsTr("Codex always uses the MCP servers set up for it."))
                        lines.push(area.mcpLine(folderItem.modelData))
                        return lines.join("\n")
                    }
                    Toggle {
                        label: qsTr("Use my MCP servers here")
                        checked: folderItem.modelData.mcp !== false
                        onToggled: (on) => Agents.setFolderMcp(folderItem.modelData.id, on)
                    }
                }
                // A restore point for each turn, in G7's own folder.
                Line {
                    visible: area.undo
                    indent: 30
                    iconPath: Icons.history
                    title: qsTr("Keep a restore point for each turn")
                    description: qsTr("Kept in G7's own folder on this PC, not in your repository. Nothing is added to .git and nothing can be pushed.")
                    Toggle {
                        label: qsTr("Keep a restore point for each turn")
                        checked: folderItem.modelData.checkpoints !== false
                        onToggled: (on) => Agents.setFolderCheckpoints(folderItem.modelData.id, on)
                    }
                }
                // Commands you run here with one click (§X5.1).
                Line {
                    visible: area.scriptsOn
                    indent: 30
                    iconPath: Icons.play
                    title: qsTr("Scripts")
                    description: qsTr("Commands you run in this folder with one click. They run on this PC as you, like in a terminal. G7 reads none from the folder itself.")
                }
                AgentScriptRows {
                    visible: area.scriptsOn
                    x: Theme.rowPadding + 60
                    width: parent.width - x - Theme.rowPadding
                    bottomPadding: 8
                    folderId: folderItem.modelData.id
                    scripts: folderItem.modelData.scripts || ({})
                }
            }
        }
        Txt {
            x: Theme.rowPadding
            width: parent.width - 2 * Theme.rowPadding
            topPadding: 6
            text: qsTr("Agents can read, change and run things in these folders, and a command you approve can reach outside them. A folder's own agent settings, such as hooks, also run, so add only folders you trust. Your phone sees only the folder names.")
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }

    // ---- Source control: the GitHub CLI, keep in step, worktrees ----
    Column {
        id: sourceSection
        visible: area.connected && area.gitSetup.available === true
        width: parent.width
        readonly property var gh: area.gitSetup.gh || ({})
        SectionLabel { width: parent.width; text: qsTr("Source control") }
        Line {
            iconPath: AgentGitGlyphs.pullRequest
            title: qsTr("GitHub CLI")
            description: {
                const gh = sourceSection.gh
                switch (gh.gh) {
                case "ok": return [qsTr("GitHub CLI %1, signed in").arg(gh.version || ""), (gh.hosts || []).join(", ")]
                    .filter((x) => x.length > 0).join(" · ")
                case "missing": return qsTr("Pull requests need the GitHub CLI.")
                case "signed_out": return qsTr("You're signed out of the GitHub CLI. Run:") + " gh auth login"
                case "old": return qsTr("Your GitHub CLI is too old for this. Update it.")
                default: return ""
                }
            }
            Button {
                variant: "text"
                size: "sm"
                text: qsTr("Check again")
                onClicked: Agents.gitSetupRead(true)
            }
        }
        Repeater {
            model: area.folders
            delegate: Line {
                id: followLine
                required property var modelData
                required property int index
                iconPath: Icons.folder
                title: modelData.label + " · " + qsTr("Keep in step with the remote")
                description: followLine.index === 0
                    ? qsTr("Fetches every few minutes while you're looking at a thread here. Uses your git sign-in.") : ""
                Toggle {
                    label: qsTr("Keep in step with the remote, %1").arg(followLine.modelData.label)
                    checked: followLine.modelData.followRemote === true
                    onToggled: (on) => Agents.setFollowRemote(followLine.modelData.id, on)
                }
            }
        }
        Txt {
            x: Theme.rowPadding
            visible: (area.gitSetup.worktrees || []).length > 0
            height: visible ? 32 : 0
            verticalAlignment: Text.AlignVCenter
            text: qsTr("Worktrees")
            role: "label"
            muted: true
        }
        Repeater {
            model: area.gitSetup.worktrees || []
            delegate: Line {
                id: worktreeLine
                required property var modelData
                iconPath: Icons.folder
                mono: true
                title: modelData.branch
                description: [modelData.folder, modelData.thread.length > 0 ? modelData.thread : qsTr("no thread")]
                    .join(" · ")
                Txt {
                    text: worktreeLine.modelData.missing === true ? qsTr("Missing")
                        : (worktreeLine.modelData.over === true ? "> " : "") + area.size(worktreeLine.modelData.bytes || 0)
                    role: "bodySmall"
                    muted: true
                }
                Button {
                    variant: "text"
                    size: "sm"
                    text: worktreeLine.modelData.missing === true ? qsTr("Forget") : qsTr("Remove")
                    onClicked: {
                        if (worktreeLine.modelData.missing === true)
                            Agents.worktreeRemove(worktreeLine.modelData.id, true)
                        else
                            area.removeWorktree(worktreeLine.modelData.id, worktreeLine.modelData.branch,
                                                worktreeLine.modelData.unpushed || 0)
                    }
                }
            }
        }
    }

    // ---- Phones: each one's three switches ----
    Column {
        visible: DeviceList.count > 0
        width: parent.width
        SectionLabel { width: parent.width; text: qsTr("Phones") }
        Repeater {
            model: DeviceList
            delegate: Column {
                id: phone
                required property string deviceId
                required property string name
                readonly property bool agentsOn: area.toggleOn(deviceId, "agents")
                width: parent.width
                Line {
                    iconPath: Icons.phone
                    iconColor: Theme.tintLavender
                    title: qsTr("%1 can use agents").arg(phone.name)
                    description: qsTr("Start threads, reply and approve. Started from the phone, a thread asks before it changes a file or runs a command that could change something.")
                    Toggle {
                        label: qsTr("%1 can use agents").arg(phone.name)
                        checked: phone.agentsOn
                        onToggled: (on) => AppController.setDeviceToggle(phone.deviceId, "agents", on)
                    }
                }
                Line {
                    indent: 30
                    title: qsTr("%1 can see sessions started on this PC").arg(phone.name)
                    description: qsTr("Sessions you start in a terminal or the Claude app, in any folder. It can read them, and continue one as a thread once it ends.")
                    Toggle {
                        enabled: phone.agentsOn
                        label: qsTr("%1 can see sessions started on this PC").arg(phone.name)
                        checked: phone.agentsOn && area.toggleOn(phone.deviceId, "agent_sessions")
                        onToggled: (on) => AppController.setDeviceToggle(phone.deviceId, "agent_sessions", on)
                    }
                }
                Line {
                    id: replyLine
                    readonly property bool sessionsOn: phone.agentsOn && area.toggleOn(phone.deviceId, "agent_sessions")
                    indent: 30
                    title: qsTr("%1 can reply to sessions on this PC").arg(phone.name)
                    description: qsTr("Brings the session's terminal window and its tab forward, pastes the reply and presses Enter. Never while the PC is locked; it waits while the PC is in use.")
                    Toggle {
                        enabled: replyLine.sessionsOn
                        label: qsTr("%1 can reply to sessions on this PC").arg(phone.name)
                        checked: replyLine.sessionsOn && area.toggleOn(phone.deviceId, "agent_reply")
                        onToggled: (on) => AppController.setDeviceToggle(phone.deviceId, "agent_reply", on)
                    }
                }
            }
        }
        // Replies need the companion extension's PRIMARY write and window
        // states, and the virtual keyboard.
        Line {
            visible: area.setup.reply === "extension" || area.setup.reply === "keyboard"
            indent: 30
            iconPath: Icons.info
            title: area.setup.reply === "keyboard"
                ? qsTr("Replies from phones can't type on this PC")
                : qsTr("Replies from phones need the G7 Linux Companion 1.5")
            description: area.setup.reply === "keyboard"
                ? qsTr("G7 Link can't make its virtual keyboard here. Allow it to use /dev/uinput.")
                : qsTr("Update or turn on the GNOME Shell extension. Until then, phones can read sessions but not reply to them.")
        }
    }

    // ---- Sessions started outside G7 ----
    Column {
        visible: area.connected
        width: parent.width
        SectionLabel { width: parent.width; text: qsTr("Sessions started outside G7") }
        Line {
            iconPath: Icons.terminal
            title: qsTr("Follow sessions started outside G7")
            description: qsTr("List Claude and Codex sessions from a terminal or the Claude app under On this PC, read from their own session files on this PC.")
            Toggle {
                enabled: area.setup.following === true || area.setup.following === false
                label: qsTr("Follow sessions started outside G7")
                checked: area.setup.following !== false
                onToggled: (on) => Agents.setFollowing(on)
            }
        }
    }

    Txt {
        x: Theme.rowPadding
        width: parent.width - 2 * Theme.rowPadding
        text: qsTr("Agents run the Claude Code and Codex you installed, signed in with your own accounts, and count against your own plan's limits. G7 is not affiliated with Anthropic or OpenAI.")
        role: "bodySmall"
        muted: true
        wrapMode: Text.WordWrap
        elide: Text.ElideNone
    }
}
