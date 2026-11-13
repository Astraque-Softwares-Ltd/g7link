// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// A short message on the raised surface just above the footer that hides
// itself, with an optional action ("Open", "Call", "Track").
Rectangle {
    id: toast
    property int timeout: 4000
    property int actionTimeout: 6500
    property string actionText: ""
    signal actionTriggered()

    function show(message) {
        label.text = message
        actionText = ""
        shown = true
        timer.interval = toast.timeout
        timer.restart()
    }

    function showWithAction(message, actionLabel) {
        label.text = message
        actionText = actionLabel || ""
        shown = true
        timer.interval = actionText.length > 0 ? toast.actionTimeout : toast.timeout
        timer.restart()
    }

    property bool shown: false
    visible: opacity > 0
    anchors.horizontalCenter: parent.horizontalCenter
    anchors.bottom: parent.bottom
    anchors.bottomMargin: Theme.footerHeight + 12
    // It rises into place as it fades in: a translation, which lays nothing out again.
    transform: Translate {
        y: toast.shown ? 0 : Theme.rise
        Behavior on y {
            NumberAnimation {
                duration: toast.shown ? Theme.smallIn : Theme.smallOut
                easing.type: Easing.BezierSpline
                easing.bezierCurve: toast.shown ? Theme.easeEnter : Theme.easeExit
            }
        }
    }
    opacity: shown ? 1 : 0
    Behavior on opacity {
        NumberAnimation {
            duration: toast.shown ? Theme.smallIn : Theme.smallOut
            easing.type: Easing.BezierSpline
            easing.bezierCurve: toast.shown ? Theme.easeEnter : Theme.easeExit
        }
    }
    z: 200

    readonly property bool hasAction: actionText.length > 0
    width: Math.min(contentRow.implicitWidth + (hasAction ? 16 : 28), parent.width - 48)
    height: 36
    radius: Theme.rowRadius
    color: Theme.raised
    border.width: 1
    border.color: Theme.line

    Accessible.role: Accessible.AlertMessage
    Accessible.name: hasAction ? (label.text + " " + actionText) : label.text

    Row {
        id: contentRow
        anchors.centerIn: parent
        spacing: 12

        Txt {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, toast.parent.width - (toast.hasAction ? actionChip.width + 100 : 88))
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
        }

        Rectangle {
            id: actionChip
            visible: toast.hasAction
            anchors.verticalCenter: parent.verticalCenter
            width: chipLabel.implicitWidth + 20
            height: 26
            radius: Theme.radiusSm
            color: chipHover.hovered ? Theme.selection : "transparent"
            activeFocusOnTab: toast.shown && toast.hasAction

            Accessible.role: Accessible.Button
            Accessible.name: toast.actionText
            Accessible.onPressAction: trigger()

            function trigger() {
                toast.shown = false
                timer.stop()
                toast.actionTriggered()
            }

            Keys.onPressed: (event) => {
                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                    trigger()
                    event.accepted = true
                }
            }

            Rectangle {
                anchors.fill: parent
                anchors.margins: -3
                radius: parent.radius + 3
                color: "transparent"
                border.width: 2
                border.color: Theme.focusRing
                visible: Theme.focusVisible(actionChip)
            }

            Txt {
                id: chipLabel
                anchors.centerIn: parent
                text: toast.actionText
                role: "label"
                color: Theme.accent
            }

            HoverHandler { id: chipHover; cursorShape: Qt.PointingHandCursor }
            TapHandler { onTapped: actionChip.trigger() }
        }
    }

    // Stays while the pointer is on it, so its action can be reached.
    HoverHandler {
        onHoveredChanged: if (hovered) timer.stop(); else if (toast.shown) timer.restart()
    }
    Timer { id: timer; interval: toast.timeout; onTriggered: toast.shown = false }
}

