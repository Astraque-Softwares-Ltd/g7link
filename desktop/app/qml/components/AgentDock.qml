// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The request the agent waits on, above the composer: one at a time ("1 of
// 3"), the folder beside the command or the change, the agent's reason as
// the agent's, Deny then Allow. With words in the composer, Allow is an
// outline and its shortcut is off ("Deny and send" is the composer's). A
// request that arrived while you were typing waits, its buttons off, until
// you pause. Its content scrolls inside; the page caps its height.
Item {
    id: dock
    // The request (`Agents.ask`, parsed).
    property var ask: ({})
    property string agentName
    property string folder
    // "Always allow …" is offered for it.
    property bool alwaysOffered: false
    // The composer has words in it.
    property bool composerHasText: false
    property bool busy: false
    // Date.now(), ticking.
    property real now: Date.now()

    signal allow
    signal deny
    signal allowThread
    signal stop

    readonly property bool held: ask.held === true
    readonly property bool isCommand: (ask.command || "").length > 0
    readonly property bool isDiff: (ask.diffDoc || "").length > 0
    readonly property string title: ask.kind === "command" ? qsTr("%1 wants to run a command").arg(agentName)
        : ask.kind === "edit" ? qsTr("%1 wants to change %2").arg(agentName).arg(ask.path || ask.title || "")
        : (ask.title || "")
    readonly property string asked: {
        const minutes = Math.max(0, Math.floor((now - (ask.created_ms || now)) / 60000))
        return minutes < 1 ? qsTr("Asked just now") : qsTr("Asked %n min ago", "", minutes)
    }
    // The overflow's actions: always allow (this PC only) and Stop.
    readonly property var overflowActions: [
        { label: ask.kind === "command" ? qsTr("Always allow this exact command in this thread")
                                        : qsTr("Always allow edits in this thread"),
          icon: Icons.shield, enabled: alwaysOffered && !held && !busy, run: () => dock.allowThread() },
        { label: qsTr("Stop"), icon: Icons.stopSquare, tint: Theme.tintRed, run: () => dock.stop() }
    ].filter((a) => a.enabled !== false)

    signal overflowRequested(real x, real y)

    implicitHeight: card.implicitHeight

    Rectangle {
        id: card
        width: parent.width
        height: parent.height
        implicitHeight: header.height + scroll.contentHeight + buttons.height + 38
        radius: Theme.radiusMd
        color: Theme.surfaceContainerLow
        border.width: 1
        border.color: Theme.outlineVariant

        // Title, "1 of 3", and the overflow.
        Item {
            id: header
            x: 12
            y: 10
            width: parent.width - 24
            height: Math.max(titleText.height, 24)
            Icon {
                id: kindIcon
                y: 3
                width: 16
                height: 16
                path: dock.ask.kind === "edit" ? Icons.edit : Icons.terminal
                color: Theme.tintAmber
            }
            Txt {
                id: titleText
                anchors.left: kindIcon.right
                anchors.leftMargin: 8
                anchors.right: count.left
                anchors.rightMargin: 8
                text: dock.title
                weight: 600
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
                maximumLineCount: 2
            }
            Txt {
                id: count
                objectName: "count"
                visible: (dock.ask.total || 0) > 1
                anchors.right: more.left
                anchors.rightMargin: 4
                y: 3
                text: qsTr("%1 of %2").arg(dock.ask.position || 1).arg(dock.ask.total || 1)
                role: "bodySmall"
                muted: true
            }
            IconButton {
                id: more
                anchors.right: parent.right
                y: -2
                width: 26
                height: 26
                iconPath: Icons.more
                label: qsTr("More")
                onClicked: {
                    const p = more.mapToItem(null, more.width, more.height)
                    dock.overflowRequested(p.x, p.y)
                }
            }
        }

        // What it wants to do: scrolls inside the dock.
        Flickable {
            id: scroll
            x: 12
            anchors.top: header.bottom
            anchors.topMargin: 8
            anchors.bottom: buttons.top
            anchors.bottomMargin: 10
            width: parent.width - 24
            contentHeight: content.height
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            Column {
                id: content
                width: scroll.width
                spacing: 10
                Rectangle {
                    visible: dock.isCommand || dock.isDiff
                    width: parent.width
                    height: visible ? inner.height + 16 : 0
                    radius: Theme.radiusSm
                    color: Theme.surfaceContainerHigh
                    Column {
                        id: inner
                        x: 10
                        y: 8
                        width: parent.width - 20
                        spacing: 6
                        // The folder it runs in.
                        Row {
                            visible: dock.folder.length > 0
                            spacing: 6
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 13
                                height: 13
                                path: Icons.folder
                                color: Theme.surfaceContentVariant
                            }
                            Txt {
                                anchors.verticalCenter: parent.verticalCenter
                                text: dock.folder
                                role: "caption"
                                muted: true
                            }
                        }
                        Txt {
                            visible: dock.isCommand
                            width: parent.width
                            text: dock.ask.command || ""
                            role: "code"
                            weight: 600
                            wrapMode: Text.WrapAnywhere
                            elide: Text.ElideNone
                        }
                        AgentDiffView {
                            visible: dock.isDiff
                            width: parent.width
                            height: visible ? implicitHeight : 0
                            diff: dock.isDiff ? dock.ask.diffDoc : ""
                            showPaths: false
                        }
                    }
                }
                // The agent's own words, set apart from G7's.
                Row {
                    visible: (dock.ask.reason || "").length > 0
                    width: parent.width
                    spacing: 8
                    Rectangle {
                        width: 2
                        height: reason.height
                        color: Theme.outlineVariant
                    }
                    Txt {
                        id: reason
                        width: parent.width - 10
                        text: qsTr("%1 says: %2").arg(dock.agentName).arg(dock.ask.reason || "")
                        role: "bodySmall"
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                }
                Txt {
                    visible: dock.ask.truncated === true
                    width: parent.width
                    text: qsTr("It's too long to show in full here.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                }
            }
        }
        G7ScrollBar {
            flickable: scroll
            anchors.top: scroll.top
            anchors.bottom: scroll.bottom
            anchors.right: scroll.right
        }

        // Deny then Allow; a held request says why its buttons wait.
        Item {
            id: buttons
            x: 12
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 10
            width: parent.width - 24
            height: 30
            Row {
                spacing: 8
                Button {
                    objectName: "deny"
                    variant: "outline"
                    size: "sm"
                    text: qsTr("Deny")
                    enabled: !dock.held && !dock.busy
                    onClicked: dock.deny()
                }
                Button {
                    id: allowButton
                    objectName: "allow"
                    variant: dock.composerHasText ? "outline" : "fill"
                    size: "sm"
                    text: qsTr("Allow")
                    enabled: !dock.held && !dock.busy
                    onClicked: dock.allow()
                }
                Keycap {
                    objectName: "allowKeys"
                    // The shortcut is off while the composer has words.
                    visible: !dock.composerHasText && !dock.held
                    anchors.verticalCenter: parent.verticalCenter
                    keys: "Ctrl Enter"
                }
            }
            Row {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Icon {
                    visible: dock.held
                    anchors.verticalCenter: parent.verticalCenter
                    width: 13
                    height: 13
                    path: Icons.keyboard
                    color: Theme.tintAmber
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: dock.held ? qsTr("Arrived while you were typing") : dock.asked
                    role: "caption"
                    color: dock.held ? Theme.tintAmber : Theme.surfaceContentVariant
                }
            }
        }
    }
}
