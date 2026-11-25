// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A plan the agent proposed, as a card in the timeline: this is its head
// ("Plan", where it stands, Copy) or its foot ("Show the whole plan",
// Change the plan, Implement with its mode menu); its blocks are the rows
// between them. Only the newest proposed plan can be implemented or
// changed, and not while a turn runs. A plan a newer one replaced shows its
// head only until opened.
Item {
    id: card
    required property string key
    required property string itemId
    required property int depth
    // "head" or "foot".
    required property string planPart
    // "proposed", "implemented" or "replaced".
    required property string status
    // Blocks left out until it's opened.
    required property int more
    required property bool isOpen
    // A turn runs or waits.
    property bool running: false

    signal implement(string itemId, string mode)
    signal implementMenuRequested(string itemId, real x, real y)
    signal changePlan

    readonly property bool head: planPart === "head"
    readonly property bool proposed: status === "proposed"
    readonly property string toggleKey: "p:" + itemId
    // Tabler's list-check (MIT, as Icons.qml credits).
    readonly property string planIcon: "M3.5 5.5l1.5 1.5 2.5-2.5M3.5 11.5l1.5 1.5 2.5-2.5M3.5 17.5l1.5 1.5 2.5-2.5M11 6h9M11 12h9M11 18h9"

    width: ListView.view ? ListView.view.width : 0
    height: head ? 44 : footColumn.height + 14

    AgentPlanFrame {
        x: card.depth * 18
        width: parent.width - x
        height: parent.height
        part: card.planPart
    }

    // ---- Head ----
    Item {
        visible: card.head
        x: card.depth * 18 + 14
        width: parent.width - x - 10
        height: parent.height
        Row {
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 16
                height: 16
                path: card.planIcon
                color: Theme.surfaceContentVariant
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Plan")
                role: "label"
                muted: true
            }
            Icon {
                visible: card.status === "implemented"
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: Icons.check
                color: AgentPalette.done
            }
            Txt {
                visible: !card.proposed
                anchors.verticalCenter: parent.verticalCenter
                text: card.status === "implemented" ? qsTr("Implemented") : qsTr("Replaced by a newer plan")
                role: "bodySmall"
                color: card.status === "implemented" ? AgentPalette.done : Theme.surfaceContentVariant
            }
        }
        IconButton {
            id: copyButton
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 28
            height: 28
            iconPath: Icons.copy
            label: qsTr("Copy")
            doneLabel: qsTr("Copied")
            onClicked: {
                if (Agents.copyText("i:" + card.itemId))
                    copyButton.flashDone()
            }
        }
        // A replaced plan opens from its head.
        MouseArea {
            anchors.left: parent.left
            anchors.right: copyButton.left
            height: parent.height
            enabled: card.status === "replaced"
            cursorShape: Qt.PointingHandCursor
            onClicked: AgentMotion.toggle(card.toggleKey)
        }
    }

    // ---- Foot ----
    Column {
        id: footColumn
        visible: !card.head
        x: card.depth * 18 + 14
        y: 4
        width: parent.width - x - 14
        spacing: 8
        TextAction {
            visible: card.more > 0 || card.isOpen
            text: card.isOpen ? qsTr("Show less") : qsTr("Show the whole plan")
            iconPath: card.isOpen ? Icons.chevronUp : Icons.chevronDown
            iconColor: Theme.surfaceContentVariant
            onClicked: AgentMotion.toggle(card.toggleKey)
        }
        Rectangle {
            visible: card.proposed
            width: parent.width
            height: 1
            color: Theme.line
        }
        Row {
            visible: card.proposed
            anchors.right: parent.right
            spacing: 8
            Button {
                objectName: "changePlan"
                variant: "outline"
                size: "sm"
                text: qsTr("Change the plan")
                enabled: !card.running
                onClicked: card.changePlan()
            }
            Row {
                spacing: 1
                Button {
                    objectName: "implement"
                    size: "sm"
                    text: qsTr("Implement")
                    enabled: !card.running
                    onClicked: card.implement(card.itemId, "supervised")
                }
                IconButton {
                    id: modeButton
                    width: 26
                    height: 30
                    iconPath: Icons.chevronDown
                    label: qsTr("How to implement it")
                    enabled: !card.running
                    onClicked: {
                        const p = modeButton.mapToItem(null, modeButton.width, 0)
                        card.implementMenuRequested(card.itemId, p.x, p.y)
                    }
                }
            }
        }
    }
}
