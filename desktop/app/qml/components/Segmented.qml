// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A segmented choice: `options` is a list of { value, label }. A value can be a
// number, as in the app size: `value` holds it as text, so they're compared as text.
Rectangle {
    id: segmented
    property var options: []
    property string value
    signal picked(string value)

    readonly property int selectedIndex: {
        const opts = options || []
        for (let i = 0; i < opts.length; i++) {
            if (String(opts[i].value) === value)
                return i
        }
        return -1
    }
    readonly property Item selectedItem: (selectedIndex >= 0 && selectedIndex < segRepeater.count)
        ? segRepeater.itemAt(selectedIndex) : null

    implicitHeight: 30
    implicitWidth: row.implicitWidth + 6
    radius: Theme.radiusFull
    color: "transparent"

    Accessible.role: Accessible.PageTabList

    // Sliding selection pill behind the segments.
    Rectangle {
        id: slideIndicator
        visible: segmented.selectedItem !== null
        x: segmented.selectedItem ? row.x + segmented.selectedItem.x : row.x
        y: 3
        width: segmented.selectedItem ? segmented.selectedItem.width : 0
        height: segmented.height - 6
        radius: Theme.radiusSm
        color: Theme.selection
        Behavior on x {
            enabled: !Theme.reduceMotion && slideIndicator.width > 0
            SpringAnimation { spring: Theme.springSnappy; damping: Theme.dampingSnappy }
        }
        Behavior on width {
            enabled: !Theme.reduceMotion && slideIndicator.width > 0
            SpringAnimation { spring: Theme.springSnappy; damping: Theme.dampingSnappy }
        }
    }

    Row {
        id: row
        anchors.centerIn: parent
        spacing: 2
        Repeater {
            id: segRepeater
            model: segmented.options
            delegate: FocusScope {
                id: option
                required property var modelData
                required property int index
                readonly property bool selected: String(modelData.value) === segmented.value
                width: label.implicitWidth + 20
                height: segmented.height - 6
                activeFocusOnTab: true
                scale: segMouse.pressed && !Theme.reduceMotion ? Theme.pressScale : 1.0
                Behavior on scale {
                    enabled: !Theme.reduceMotion
                    NumberAnimation { duration: Theme.quickIn; easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeMove }
                }
                Accessible.role: Accessible.PageTab
                Accessible.name: modelData.label
                Accessible.selected: selected
                Accessible.onPressAction: segmented.picked(modelData.value)
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                        segmented.picked(modelData.value)
                        event.accepted = true
                    }
                }
                Rectangle {
                    anchors.fill: parent
                    radius: Theme.radiusFull
                    color: "transparent"
                    Rectangle {
                        anchors.fill: parent
                        anchors.margins: -3
                        radius: parent.radius + 3
                        color: "transparent"
                        border.width: 2
                        border.color: Theme.focusRing
                        visible: Theme.focusVisible(option)
                    }
                }
                Txt {
                    id: label
                    anchors.centerIn: parent
                    text: option.modelData.label
                    role: "bodySmall"
                    color: option.selected
                        ? Theme.surfaceContent
                        : (segMouse.containsMouse ? Theme.surfaceContent : Theme.surfaceContentVariant)
                    Behavior on color { enabled: !Theme.reduceMotion; ColorAnimation { duration: Theme.fadeFast } }
                }
                MouseArea {
                    id: segMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: segmented.picked(option.modelData.value)
                }
            }
        }
    }
}
