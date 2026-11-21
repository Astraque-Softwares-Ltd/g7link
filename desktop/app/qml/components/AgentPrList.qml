// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Pull requests (§SC4.4), read through the GitHub CLI: Mine, Review
// requested, and This folder (with each one's checks). Read-only: a row
// opens in the browser, opens its thread, or copies its link.
Item {
    id: list
    // Agents.prs: { show, folderId, loading, refreshing, failed, readMs, gh: { gh, … }, more, rows: [...] }
    property var prs: ({})
    property var folders: []
    property real now: Date.now()
    signal openThread(string id)

    readonly property string show: prs.show || "mine"
    readonly property var rows: prs.rows || []
    readonly property string gh: (prs.gh || {}).gh || ""
    readonly property bool blocked: gh === "missing" || gh === "signed_out" || gh === "old"

    // A command to run in a terminal, with Copy.
    component CommandLine: Rectangle {
        id: commandLine
        property string command
        height: 32
        radius: Theme.radiusSm
        color: AgentPalette.codeGround
        Txt {
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 48
            text: commandLine.command
            role: "code"
            size: Theme.typography.bodySmall.size
            maximumLineCount: 1
        }
        IconButton {
            anchors.right: parent.right
            anchors.rightMargin: 2
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.copy
            iconSize: 14
            label: qsTr("Copy")
            onClicked: Agents.copy(commandLine.command)
        }
    }

    Item {
        id: bar
        width: parent.width
        height: 48
        Segmented {
            id: tabs
            x: 16
            anchors.verticalCenter: parent.verticalCenter
            options: [
                { value: "mine", label: qsTr("Mine") },
                { value: "review", label: qsTr("Review requested") },
                { value: "folder", label: qsTr("This folder") }
            ]
            value: list.show
            onPicked: (value) => Agents.prsOpen(value, value === "folder" ? (list.prs.folderId || "") : "")
        }
        Chip {
            id: folderChip
            visible: list.show === "folder" && list.folders.length > 1
            anchors.left: tabs.right
            anchors.leftMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            interactive: true
            iconPath: Icons.folder
            text: (list.folders.find((f) => f.id === list.prs.folderId) || {}).label || ""
            onClicked: {
                const i = list.folders.findIndex((f) => f.id === list.prs.folderId)
                const next = list.folders[(i + 1) % list.folders.length]
                if (next)
                    Agents.prsOpen("folder", next.id)
            }
        }
        Txt {
            anchors.right: refresh.left
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            visible: (list.prs.readMs || 0) > 0
            text: qsTr("Read %1").arg(new Date(list.prs.readMs || 0).toLocaleTimeString(Qt.locale(), Locale.ShortFormat))
            role: "caption"
            muted: true
        }
        IconButton {
            id: refresh
            anchors.right: parent.right
            anchors.rightMargin: 12
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.refresh
            label: qsTr("Refresh")
            enabled: list.prs.refreshing !== true
            onClicked: Agents.prsRefresh()
        }
    }
    Rectangle {
        id: barLine
        anchors.top: bar.bottom
        width: parent.width
        height: 1
        color: Theme.line
    }
    ListView {
        id: view
        visible: !list.blocked && list.rows.length > 0
        anchors.top: barLine.bottom
        anchors.topMargin: 6
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: list.rows
        delegate: AgentPrRow {
            required property var modelData
            width: view.width
            pr: modelData
            now: list.now
            onOpenThread: (id) => list.openThread(id)
        }
        footer: Txt {
            visible: list.prs.more === true
            x: 12
            height: visible ? 32 : 0
            verticalAlignment: Text.AlignVCenter
            text: qsTr("Showing the newest 50.")
            role: "bodySmall"
            muted: true
        }
        TouchpadGlide { view: view }
    }
    // Loading, nothing open, no GitHub CLI, signed out.
    Column {
        visible: !view.visible
        anchors.centerIn: parent
        width: Math.min(420, parent.width - 48)
        spacing: 10
        Spinner {
            visible: list.prs.loading === true
            anchors.horizontalCenter: parent.horizontalCenter
            width: 18
            height: 18
        }
        Txt {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: list.gh === "missing" ? qsTr("Pull requests need the GitHub CLI.")
                : list.gh === "signed_out" ? qsTr("You're signed out of the GitHub CLI. Run:")
                : list.gh === "old" ? qsTr("Your GitHub CLI is too old for this. Update it.")
                : list.prs.loading === true ? qsTr("Asking GitHub…")
                : list.prs.failed === true ? qsTr("GitHub couldn't be reached. Try again.")
                : list.show === "review" ? qsTr("Nothing is waiting for your review.")
                : qsTr("No open pull requests.")
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
        }
        Txt {
            visible: list.gh === "missing"
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: qsTr("Install it, then run:")
            role: "bodySmall"
            muted: true
        }
        CommandLine {
            visible: list.gh === "missing" || list.gh === "signed_out"
            anchors.horizontalCenter: parent.horizontalCenter
            width: Math.min(240, parent.width)
            command: "gh auth login"
        }
    }
}
