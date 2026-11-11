// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A floating list of actions over the window: the footer's Actions (Ctrl B),
// the G7 menu and the phone picker. Each action is
// `{ label, detail, icon, tint, keys, checked, enabled, run }`; only `label`
// and `run` are needed. Arrows move, Enter runs, Esc or a click outside
// closes; with `filterable`, typing narrows the list.
Item {
    id: menu
    property bool opened: false
    property var actions: []
    property bool filterable: false
    property string placeholder: qsTr("Search actions")
    property real panelWidth: 320
    property real anchorX: 0
    property real anchorY: 0
    property bool upward: true
    property bool alignRight: false
    property int current: 0
    readonly property var shown: {
        const q = filter.text.trim().toLowerCase()
        const list = (actions || []).filter((a) => a && a.label)
        if (q.length === 0)
            return list
        return list.filter((a) => a.label.toLowerCase().includes(q)
                                  || (a.detail || "").toLowerCase().includes(q))
    }

    function openAt(x, y, up, right) {
        anchorX = x
        anchorY = y
        upward = up
        alignRight = right
        filter.text = ""
        current = 0
        opened = true
        keyScope.forceActiveFocus()
    }
    function close() { opened = false }
    function run(i) {
        const a = shown[i]
        if (!a || a.enabled === false)
            return
        close()
        if (typeof a.run === "function")
            a.run()
    }

    anchors.fill: parent
    visible: opened
    z: 140
    onShownChanged: current = Math.min(current, Math.max(0, shown.length - 1))

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        onPressed: menu.close()
        onWheel: (wheel) => wheel.accepted = true
    }

    Rectangle {
        id: panel
        width: Math.min(menu.panelWidth, menu.width - 24)
        height: Math.min(content.implicitHeight + 12, menu.height - 24)
        x: Math.max(12, Math.min(menu.width - width - 12, menu.alignRight ? menu.anchorX - width : menu.anchorX))
        y: Math.max(12, Math.min(menu.height - height - 12, menu.upward ? menu.anchorY - height - 6 : menu.anchorY + 6))
        radius: Theme.radiusXl
        color: Theme.surface
        border.width: 1
        border.color: Theme.line

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
                    menu.current = Math.min(menu.shown.length - 1, menu.current + 1)
                    list.positionViewAtIndex(menu.current, ListView.Contain)
                } else if (event.key === Qt.Key_Up) {
                    menu.current = Math.max(0, menu.current - 1)
                    list.positionViewAtIndex(menu.current, ListView.Contain)
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    menu.run(menu.current)
                } else {
                    return
                }
                event.accepted = true
            }

            Column {
                id: content
                width: parent.width
                spacing: 4
                Field {
                    id: filter
                    visible: menu.filterable
                    width: parent.width
                    focus: menu.filterable
                    placeholder: menu.placeholder
                    iconPath: Icons.search
                    onAccepted: menu.run(menu.current)
                }
                ListView {
                    id: list
                    width: parent.width
                    height: Math.min(contentHeight, menu.height - 48 - (filter.visible ? filter.height + 4 : 0))
                    clip: true
                    interactive: contentHeight > height
                    boundsBehavior: Flickable.StopAtBounds
                    model: menu.shown
                    delegate: ActionRow {
                        required property var modelData
                        required property int index
                        width: ListView.view.width
                        iconPath: modelData.icon || ""
                        tint: modelData.tint !== undefined ? modelData.tint : Theme.surfaceContentVariant
                        title: modelData.label
                        detail: modelData.detail || ""
                        keys: modelData.keys || ""
                        enabled: modelData.enabled !== false
                        selected: index === menu.current
                        activeFocusOnTab: false
                        onClicked: menu.run(index)
                        Icon {
                            visible: modelData.checked === true
                            width: 16; height: 16
                            path: Icons.check
                            color: Theme.accent
                        }
                    }
                }
                Txt {
                    visible: menu.shown.length === 0
                    x: Theme.rowPadding
                    height: Theme.rowHeight
                    text: qsTr("Nothing matches")
                    muted: true
                }
            }
        }
    }
}
