// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// `/` at the start of the composer: G7's own three first (they act at once
// and send nothing), then the commands the agent reported. Choosing one of
// the agent's inserts it, with its arguments shown after it; sending it is
// a separate Enter.
Rectangle {
    id: list
    // `Agents.popup` (kind "commands"), parsed.
    property var popup: ({})
    // The thread's agent: Claude thinks, Codex reasons.
    property string agentId
    signal chosen(int index)

    readonly property var rows: popup.rows || []
    readonly property int selected: popup.selected || 0

    function ownNote(name) {
        switch (name) {
        case "model": return qsTr("Choose the model")
        case "effort": return agentId === "codex" ? qsTr("Choose how much it reasons") : qsTr("Choose how much it thinks")
        case "plan": return qsTr("Plan: it changes nothing until you say so")
        default: return ""
        }
    }

    implicitHeight: Math.min(column.implicitHeight, 300) + title.height + legend.height + 18
    radius: Theme.radiusLg
    color: Theme.surface
    border.width: 1
    border.color: Theme.line

    Accessible.role: Accessible.List
    Accessible.name: qsTr("Commands")

    Txt {
        id: title
        x: 14
        y: 6
        height: 24
        verticalAlignment: Text.AlignVCenter
        text: qsTr("Commands")
        role: "label"
        muted: true
    }
    Flickable {
        id: scroll
        x: 6
        anchors.top: title.bottom
        anchors.bottom: legend.top
        width: parent.width - 12
        contentHeight: column.implicitHeight
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        Column {
            id: column
            width: scroll.width
            spacing: 2
            Repeater {
                model: list.rows
                delegate: Column {
                    id: commandRow
                    required property var modelData
                    required property int index
                    readonly property bool groupStart: index === 0
                        || list.rows[index - 1].own !== modelData.own
                    width: column.width
                    Txt {
                        visible: commandRow.groupStart
                        x: 8
                        height: visible ? 22 : 0
                        verticalAlignment: Text.AlignVCenter
                        text: commandRow.modelData.own ? "G7" : (list.popup.agentName || "")
                        role: "caption"
                        muted: true
                    }
                    Rectangle {
                        width: parent.width
                        height: 28
                        radius: Theme.radiusSm
                        color: commandRow.index === list.selected ? Theme.surfaceContainerHigh : "transparent"
                        Accessible.role: Accessible.ListItem
                        Accessible.name: "/" + commandRow.modelData.name
                        Accessible.selected: commandRow.index === list.selected
                        Txt {
                            id: nameText
                            x: 8
                            anchors.verticalCenter: parent.verticalCenter
                            text: "/" + commandRow.modelData.name
                            role: "code"
                            weight: 600
                        }
                        Txt {
                            anchors.left: nameText.right
                            anchors.leftMargin: 10
                            anchors.right: hintText.left
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            text: commandRow.modelData.own ? list.ownNote(commandRow.modelData.name)
                                                           : commandRow.modelData.note
                            role: "bodySmall"
                            muted: true
                        }
                        Txt {
                            id: hintText
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            text: commandRow.modelData.hint
                            role: "caption"
                            muted: true
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: list.chosen(commandRow.index)
                        }
                    }
                }
            }
            Row {
                visible: list.rows.length === 0
                x: 8
                height: 28
                spacing: 8
                Spinner {
                    visible: list.popup.loading === true
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                }
                Txt {
                    visible: list.popup.loading !== true
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("No commands match")
                    role: "bodySmall"
                    muted: true
                }
            }
        }
    }
    G7ScrollBar {
        flickable: scroll
        anchors.top: scroll.top
        anchors.bottom: scroll.bottom
        anchors.right: scroll.right
    }
    Item {
        id: legend
        x: 6
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 6
        width: parent.width - 12
        height: 30
        Rectangle {
            width: parent.width
            height: 1
            color: Theme.line
        }
        AgentPopupLegend {
            x: 8
            anchors.verticalCenter: parent.verticalCenter
        }
    }
}
