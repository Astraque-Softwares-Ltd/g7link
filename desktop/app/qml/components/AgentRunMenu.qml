// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The Run menu (§X5.1): the folder's scripts, each with its command in mono,
// and Run, or a ring and Stop while it runs here. Its foot opens Setup at
// the folder. A script runs only when this menu's Run is pressed.
Item {
    id: menu
    // Agents.runs' scripts: [{ id, name, command, run, running }].
    property var scripts: []
    property bool opened: false
    property real anchorX: 0
    property real anchorY: 0
    property int current: 0
    // Setup, at this folder: "Edit scripts…" or "Add a script…".
    signal editRequested

    function openAt(x, y) {
        anchorX = x
        anchorY = y
        current = 0
        opened = true
        keyScope.forceActiveFocus()
    }
    function close() { opened = false }
    function act(i) {
        const s = scripts[i]
        if (!s)
            return
        if (s.running)
            Agents.scriptStop(s.run)
        else
            Agents.scriptRun(s.id)
        close()
    }

    anchors.fill: parent
    visible: opacity > 0
    enabled: opened
    opacity: opened ? 1 : 0
    z: 140
    Behavior on opacity {
        NumberAnimation {
            duration: menu.opened ? Theme.smallIn : Theme.smallOut
            easing.type: Easing.BezierSpline
            easing.bezierCurve: menu.opened ? Theme.easeEnter : Theme.easeExit
        }
    }
    onScriptsChanged: current = Math.min(current, Math.max(0, scripts.length - 1))

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        onPressed: menu.close()
        onWheel: (wheel) => wheel.accepted = true
    }

    Rectangle {
        id: panel
        width: Math.min(380, menu.width - 24)
        height: Math.min(content.implicitHeight + 12, menu.height - 24)
        x: Math.max(12, Math.min(menu.width - width - 12, menu.anchorX - width))
        y: Math.max(12, Math.min(menu.height - height - 12, menu.anchorY + 6))
        radius: Theme.radiusXl
        color: Theme.surface
        border.width: 1
        border.color: Theme.line
        transformOrigin: Item.TopRight
        scale: menu.opened ? 1 : Theme.popScale
        Behavior on scale {
            NumberAnimation {
                duration: menu.opened ? Theme.smallIn : Theme.smallOut
                easing.type: Easing.BezierSpline
                easing.bezierCurve: menu.opened ? Theme.easeEnter : Theme.easeExit
            }
        }

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
        }

        FocusScope {
            id: keyScope
            anchors.fill: parent
            anchors.margins: 6
            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_Escape) {
                    menu.close()
                } else if (event.key === Qt.Key_Down) {
                    menu.current = Math.min(menu.scripts.length - 1, menu.current + 1)
                    list.positionViewAtIndex(menu.current, ListView.Contain)
                } else if (event.key === Qt.Key_Up) {
                    menu.current = Math.max(0, menu.current - 1)
                    list.positionViewAtIndex(menu.current, ListView.Contain)
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    menu.act(menu.current)
                } else {
                    return
                }
                event.accepted = true
            }

            Column {
                id: content
                width: parent.width
                spacing: 4
                ListView {
                    id: list
                    visible: menu.scripts.length > 0
                    width: parent.width
                    height: Math.min(contentHeight, menu.height - 96)
                    clip: true
                    interactive: contentHeight > height
                    boundsBehavior: Flickable.StopAtBounds
                    model: menu.scripts
                    delegate: Rectangle {
                        id: row
                        required property var modelData
                        required property int index
                        width: ListView.view.width
                        height: Math.max(48, texts.implicitHeight + 14)
                        radius: Theme.radiusMd
                        color: row.index === menu.current || rowHover.hovered ? Theme.selection : "transparent"
                        Accessible.role: Accessible.Button
                        Accessible.name: row.modelData.name
                        Accessible.description: row.modelData.command
                        Accessible.onPressAction: menu.act(row.index)
                        HoverHandler {
                            id: rowHover
                            onHoveredChanged: if (hovered) menu.current = row.index
                        }
                        Column {
                            id: texts
                            x: 10
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - x - action.width - 18
                            spacing: 2
                            Txt {
                                width: parent.width
                                text: row.modelData.name
                                role: "body"
                                weight: 500
                                maximumLineCount: 1
                            }
                            Txt {
                                width: parent.width
                                text: row.modelData.command
                                role: "code"
                                size: Theme.typography.caption.size
                                muted: true
                                maximumLineCount: 1
                                elide: Text.ElideRight
                            }
                        }
                        Row {
                            id: action
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 8
                            Spinner {
                                visible: row.modelData.running === true
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                            }
                            Button {
                                anchors.verticalCenter: parent.verticalCenter
                                size: "sm"
                                variant: "tonal"
                                activeFocusOnTab: false
                                iconPath: row.modelData.running === true ? Icons.stopSquare : Icons.play
                                text: row.modelData.running === true ? qsTr("Stop") : qsTr("Run")
                                onClicked: menu.act(row.index)
                            }
                        }
                    }
                    TouchpadGlide { view: list }
                }
                Txt {
                    visible: menu.scripts.length === 0
                    x: 10
                    width: parent.width - 20
                    topPadding: 6
                    text: qsTr("No scripts for this folder yet.")
                    role: "bodySmall"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
                Rectangle {
                    visible: menu.scripts.length > 0
                    width: parent.width
                    height: 1
                    color: Theme.line
                }
                TextAction {
                    x: 4
                    iconPath: menu.scripts.length > 0 ? Icons.edit : Icons.plus
                    text: menu.scripts.length > 0 ? qsTr("Edit scripts…") : qsTr("Add a script…")
                    onClicked: {
                        menu.close()
                        menu.editRequested()
                    }
                }
            }
        }
    }
}
