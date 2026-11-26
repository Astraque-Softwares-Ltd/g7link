// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The modes an agent offers, as its own CLI lists them (Claude Code's mode
// menu): each row its icon, its name, a line under it in the agent's words
// and its number key, a check on the one in use. Bypass permissions and
// Full access are drawn in the warning tint. A row the agent can't use now
// says why. Up and Down move, Enter or 1 to 9 pick, Esc or a click outside
// closes. Over the page, it opens over (or under) the control that opened it.
Item {
    id: menu
    // `picker.modes` (or `implementModes`): [{ id, name, line, on, why, checked, warn }].
    property var modes: []
    // A line over the rows ("Implement in"); none when empty.
    property string heading
    // A line under them ("Applies to the next message.").
    property string footnote
    property bool opened: false
    property real anchorX: 0
    property real anchorY: 0
    property bool upward: true
    property bool alignRight: false
    // The height of the control it opens from: with it, the menu opens the
    // other way when there's no room on the side asked for.
    property real anchorHeight: 0
    property int current: 0
    signal picked(string mode)

    // A mode's glyph, as the composer's chip shows it.
    function iconFor(mode) {
        switch (mode) {
        case "accept_edits": return Icons.edit
        case "auto": return Icons.sparkle
        case "plan": return Icons.clipboardList
        case "full": return Icons.bolt
        default: return Icons.shield
        }
    }
    function openAt(x, y, up, right, controlHeight) {
        anchorX = x
        anchorY = y
        upward = up
        alignRight = right
        anchorHeight = controlHeight || 0
        const at = modes.findIndex((m) => m.checked === true)
        current = Math.max(0, at)
        opened = true
        keyScope.forceActiveFocus()
    }
    function close() { opened = false }
    function pick(i) {
        const m = modes[i]
        if (!m || m.on === false)
            return
        close()
        menu.picked(m.id)
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

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        onPressed: menu.close()
        onWheel: (wheel) => wheel.accepted = true
    }

    // The widest name or line, so each stays on one line while the window allows.
    Column {
        id: measure
        visible: false
        Repeater {
            model: menu.modes
            delegate: Txt {
                required property var modelData
                text: modelData.name
                weight: 600
            }
        }
        Repeater {
            model: menu.modes
            delegate: Txt {
                required property var modelData
                text: modelData.on === false && (modelData.why || "").length > 0 ? modelData.why : modelData.line
                role: "caption"
            }
        }
        Txt {
            text: menu.heading
            role: "label"
        }
    }

    Rectangle {
        id: panel
        objectName: "modePanel"
        // The glyph, the text, the check and the key, and the margins.
        width: Math.min(Math.max(300, measure.implicitWidth + 112), menu.width - 24)
        height: Math.min(column.implicitHeight + 12, menu.height - 24)
        // Opens the other way when the side asked for has no room and the other has.
        readonly property real above: (menu.upward ? menu.anchorY : menu.anchorY - menu.anchorHeight) - height - 6
        readonly property real below: (menu.upward ? menu.anchorY + menu.anchorHeight : menu.anchorY) + 6
        readonly property bool up: menu.anchorHeight <= 0 ? menu.upward
            : menu.upward ? (above >= 12 || below + height > menu.height - 12)
            : !(below + height <= menu.height - 12 || above < 12)
        x: Math.max(12, Math.min(menu.width - width - 12, menu.alignRight ? menu.anchorX - width : menu.anchorX))
        y: Math.max(12, Math.min(menu.height - height - 12, up ? above : below))
        radius: Theme.radiusXl
        color: Theme.surface
        border.width: 1
        border.color: Theme.line
        transformOrigin: up ? (menu.alignRight ? Item.BottomRight : Item.BottomLeft)
                            : (menu.alignRight ? Item.TopRight : Item.TopLeft)
        scale: menu.opened ? 1 : Theme.popScale
        Behavior on scale {
            NumberAnimation {
                duration: menu.opened ? Theme.smallIn : Theme.smallOut
                easing.type: Easing.BezierSpline
                easing.bezierCurve: menu.opened ? Theme.easeEnter : Theme.easeExit
            }
        }
        clip: true

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
        }

        FocusScope {
            id: keyScope
            anchors.fill: parent
            anchors.margins: 6
            Keys.onPressed: (event) => {
                const n = event.key - Qt.Key_1
                if (event.key === Qt.Key_Escape) {
                    menu.close()
                } else if (event.key === Qt.Key_Down) {
                    menu.current = Math.min(menu.modes.length - 1, menu.current + 1)
                } else if (event.key === Qt.Key_Up) {
                    menu.current = Math.max(0, menu.current - 1)
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    menu.pick(menu.current)
                } else if (n >= 0 && n < Math.min(9, menu.modes.length) && event.modifiers === Qt.NoModifier) {
                    menu.pick(n)
                } else {
                    return
                }
                event.accepted = true
            }

            Column {
                id: column
                width: parent.width
                spacing: 2
                Txt {
                    visible: menu.heading.length > 0
                    x: 10
                    width: parent.width - 20
                    height: 28
                    text: menu.heading
                    role: "label"
                    muted: true
                }
                Repeater {
                    model: menu.modes
                    delegate: Rectangle {
                        id: row
                        required property var modelData
                        required property int index
                        readonly property bool on: modelData.on !== false
                        readonly property color tint: modelData.warn === true ? Theme.tintAmber : Theme.surfaceContent
                        objectName: "mode_" + modelData.id
                        width: column.width
                        height: Math.max(44, rowText.implicitHeight + 14)
                        radius: Theme.radiusSm
                        color: menu.current === index ? Theme.activeFill
                            : (rowHover.hovered && on ? Theme.raised : "transparent")
                        opacity: on ? 1 : 0.55

                        Accessible.role: Accessible.RadioButton
                        Accessible.name: modelData.name
                        Accessible.description: on ? modelData.line : modelData.why
                        Accessible.checked: modelData.checked === true
                        Accessible.onPressAction: menu.pick(index)

                        Icon {
                            id: glyph
                            x: 10
                            y: 13
                            width: 16
                            height: 16
                            path: menu.iconFor(row.modelData.id)
                            color: row.modelData.warn === true ? Theme.tintAmber : Theme.surfaceContentVariant
                        }
                        Column {
                            id: rowText
                            x: 36
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - x - trailing.width - 18
                            spacing: 1
                            Txt {
                                width: parent.width
                                text: row.modelData.name
                                color: row.tint
                                weight: row.modelData.checked === true ? 600 : Theme.typography.body.weight
                                maximumLineCount: 1
                            }
                            Txt {
                                width: parent.width
                                text: row.on || (row.modelData.why || "").length === 0 ? row.modelData.line
                                                                                         : row.modelData.why
                                role: "caption"
                                muted: true
                                wrapMode: Text.WordWrap
                                elide: Text.ElideNone
                                maximumLineCount: 2
                            }
                        }
                        Row {
                            id: trailing
                            anchors.right: parent.right
                            anchors.rightMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 8
                            Icon {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 14
                                height: 14
                                opacity: row.modelData.checked === true ? 1 : 0
                                path: Icons.check
                                color: Theme.accent
                            }
                            Keycap {
                                anchors.verticalCenter: parent.verticalCenter
                                keys: row.index < 9 ? String(row.index + 1) : ""
                            }
                        }
                        HoverHandler {
                            id: rowHover
                            enabled: row.on
                        }
                        MouseArea {
                            anchors.fill: parent
                            enabled: row.on
                            cursorShape: Qt.PointingHandCursor
                            onClicked: menu.pick(row.index)
                        }
                    }
                }
                Txt {
                    visible: menu.footnote.length > 0
                    x: 10
                    width: parent.width - 20
                    height: 26
                    text: menu.footnote
                    role: "caption"
                    muted: true
                }
            }
        }
    }
}
