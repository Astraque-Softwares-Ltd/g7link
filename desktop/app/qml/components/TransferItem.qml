// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// One file transfer: direction, name, progress, speed and time left, and
// what can be done with it (cancel while it runs; open or show what arrived).
Item {
    id: item
    required property string transferId
    required property string deviceName
    required property bool incoming
    required property bool recording
    required property string title
    required property real total
    required property real done
    required property real progress
    required property real rate
    required property string status
    required property string reason
    required property string savedPath

    readonly property bool active: status === "requested" || status === "waiting" || status === "running" || status === "paused"
    readonly property bool showBar: status === "waiting" || status === "running" || status === "paused"
    readonly property bool motion: !Theme.reduceMotion && item.visible
    // Progress as drawn. Updates arrive a few times a second; easing each one
    // over about the same time keeps the bar gliding instead of stepping.
    property real shown: status === "done" ? 1 : Math.max(0, Math.min(1, progress))
    Behavior on shown { enabled: item.motion; NumberAnimation { duration: 260 } }

    implicitHeight: Math.max(Theme.rowHeight, column.height + 12)

    function size(bytes) {
        if (bytes < 1024) return qsTr("%1 B").arg(Math.round(bytes))
        const units = [qsTr("%1 KB"), qsTr("%1 MB"), qsTr("%1 GB"), qsTr("%1 TB")]
        let value = bytes / 1024
        let unit = 0
        while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++ }
        return units[unit].arg(value < 10 ? Qt.locale().toString(value, "f", 1) : Math.round(value))
    }

    // Rounded so it doesn't flicker with every speed sample.
    function timeLeft() {
        if (status !== "running" || rate <= 0 || total <= done) return ""
        const seconds = (total - done) / rate
        if (seconds < 10) return qsTr("A few seconds left")
        if (seconds < 60) return qsTr("%1 s left").arg(Math.ceil(seconds / 5) * 5)
        if (seconds < 3600) return qsTr("%1 min left").arg(Math.ceil(seconds / 60))
        return qsTr("%1 h %2 min left").arg(Math.floor(seconds / 3600)).arg(Math.floor((seconds % 3600) / 60))
    }

    function detail() {
        switch (status) {
        case "requested":
            return qsTr("%1 wants to send this · %2")
                .arg(item.deviceName.length > 0 ? item.deviceName : qsTr("Nearby device"))
                .arg(size(total))
        case "waiting":
            return item.incoming ? qsTr("Paused; continues when %1 is back").arg(item.deviceName)
                                 : qsTr("Waiting for %1").arg(item.deviceName)
        case "running": {
            const left = timeLeft()
            const sizes = rate > 0
                ? qsTr("%1 of %2 · %3/s").arg(size(done)).arg(size(total)).arg(size(rate))
                : qsTr("%1 of %2").arg(size(done)).arg(size(total))
            return left.length > 0 ? sizes + " · " + left : sizes
        }
        case "paused":
            return qsTr("Paused · %1 of %2").arg(size(done)).arg(size(total))
        case "done":
            return item.incoming ? qsTr("From %1 · %2").arg(item.deviceName).arg(size(total))
                                 : qsTr("Sent to %1 · %2").arg(item.deviceName).arg(size(total))
        case "cancelled":
            return qsTr("Cancelled")
        default:
            switch (reason) {
            case "denied": return qsTr("%1 doesn't accept files from this PC").arg(item.deviceName)
            case "unreachable": return qsTr("Couldn't reach %1").arg(item.deviceName)
            case "noSpace": return qsTr("Not enough space on %1").arg(item.deviceName)
            case "integrity": return qsTr("Integrity check failed · partial file kept to resume")
            default: return qsTr("Didn't go through")
            }
        }
    }

    Row {
        id: row
        x: Theme.rowPadding
        y: 6
        width: parent.width - 2 * Theme.rowPadding
        spacing: 10
        Icon {
            y: 2
            width: 18; height: 18
            path: item.incoming ? (item.recording ? Icons.mic : Icons.download) : Icons.send
            color: item.recording ? Theme.tintRed : Theme.tintAmber
        }
        Column {
            id: column
            width: row.width - 18 - actions.width - row.spacing * 2
            spacing: 4
            Item {
                width: parent.width
                height: titleText.height
                Txt {
                    id: titleText
                    width: parent.width - (percent.visible ? percent.width + 8 : 0)
                    text: item.title
                }
                Txt {
                    id: percent
                    anchors.right: parent.right
                    anchors.verticalCenter: titleText.verticalCenter
                    visible: item.showBar && item.total > 0
                    role: "bodySmall"
                    color: item.status === "running" ? Theme.tintAmber : Theme.surfaceContentVariant
                    text: qsTr("%1%").arg(Math.floor(item.shown * 100))
                }
            }
            // One bar for the whole transfer: it fills as it goes, dims while
            // waiting or paused, and folds away when it's done.
            Rectangle {
                width: parent.width
                height: item.showBar ? 4 : 0
                visible: height > 0
                radius: 2
                color: Theme.selection
                Accessible.role: Accessible.ProgressBar
                Accessible.name: item.title
                Accessible.description: percent.text
                Behavior on height { enabled: item.motion; NumberAnimation { duration: Theme.smallIn; easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeMove } }
                Rectangle {
                    width: item.shown <= 0 ? 0 : Math.max(parent.height, parent.width * item.shown)
                    height: parent.height
                    radius: parent.radius
                    color: Theme.tintAmber
                    opacity: item.status === "waiting" || item.status === "paused" ? 0.5 : 1
                }
            }
            Txt {
                width: parent.width
                role: "bodySmall"
                color: item.status === "failed" ? Theme.tintRed : Theme.surfaceContentVariant
                text: item.detail()
            }
        }
        Row {
            id: actions
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4
            Button {
                visible: item.status === "requested"
                size: "sm"
                text: qsTr("Accept")
                onClicked: TransferList.accept(item.transferId)
            }
            Button {
                visible: item.status === "requested"
                variant: "outline"
                size: "sm"
                text: qsTr("Decline")
                onClicked: TransferList.cancel(item.transferId)
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                visible: item.status === "done" && item.incoming && item.savedPath.length > 0
                text: qsTr("Open")
                onClicked: TransferList.open(item.transferId)
            }
            IconButton {
                visible: item.status === "done" && item.incoming && item.savedPath.length > 0
                iconPath: Icons.folder
                label: qsTr("Show in folder")
                onClicked: TransferList.showInFolder(item.transferId)
            }
            IconButton {
                visible: item.status === "waiting" || item.status === "running"
                iconPath: Icons.pause
                label: qsTr("Pause")
                onClicked: TransferList.pause(item.transferId)
            }
            IconButton {
                visible: item.status === "paused"
                iconPath: Icons.play
                label: qsTr("Resume")
                onClicked: TransferList.resume(item.transferId)
            }
            IconButton {
                visible: item.status === "waiting" || item.status === "running" || item.status === "paused"
                iconPath: Icons.close
                label: qsTr("Cancel")
                onClicked: TransferList.cancel(item.transferId)
            }
        }
    }
}
