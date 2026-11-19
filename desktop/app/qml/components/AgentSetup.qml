// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import app.g7link

// The Agents page's setup: the agent service, the agents and how they're
// signed in, the folders they may work in, each phone's two switches, and
// following sessions started outside G7. No row has a free-width slot: text
// takes the room left by fixed controls, paths are one line cut in the
// middle (the whole path shows on hover), and commands wrap anywhere.
Column {
    id: area
    // Agents.setup, parsed.
    property var setup: ({})
    signal addFolderRequested

    readonly property bool connected: setup.status === "connected"
    readonly property var agents: connected ? (setup.agents || []) : []
    readonly property var folders: connected ? (setup.folders || []) : []

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
                description: needsLogin
                    ? qsTr("Sign in by running %1 in a terminal.").arg(modelData.loginHint)
                    : area.agentDetail(modelData)
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

    // ---- Folders ----
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
            model: area.folders
            delegate: Line {
                id: folderLine
                required property var modelData
                iconPath: Icons.folder
                iconColor: modelData.exists ? Theme.tintAmber : Theme.surfaceContentVariant
                title: modelData.shownPath
                fullText: modelData.path
                mono: true
                description: modelData.exists ? "" : qsTr("Not found")
                Accessible.name: modelData.path
                Button {
                    variant: "text"
                    size: "sm"
                    iconPath: Icons.trash
                    text: qsTr("Remove")
                    onClicked: Agents.removeFolder(folderLine.modelData.id)
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

    // ---- Phones: each one's two switches ----
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
            }
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
