// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Pairing a phone: a QR code to scan (refreshed automatically) beside the
// phones nearby, then the 6-digit code both screens show, and the outcome.
// While `running`, this PC is in pairing mode. Side by side when there's
// room, stacked when not.
Item {
    id: panel
    property bool running: false
    // Show "Cancel" (in a dialog) or not (first run).
    property bool cancellable: true
    property bool compact: false
    signal finished
    signal cancelled

    readonly property string state_: Pairing.state
    readonly property bool hosting: state_ === "hosting" || state_ === "starting" || state_ === "idle"
    readonly property bool wide: width >= 600
    // The left side shows the Android app's download QR instead.
    property bool showApp: false
    // Comparing codes (or waiting on the phone after a match): a compact
    // block with just the code and its two answers.
    readonly property bool checking: state_ === "comparing" || state_ === "confirmed"
    readonly property string releaseUrl: "https://github.com/Astraque-Softwares-Ltd/g7link/releases/latest"

    implicitWidth: 760
    implicitHeight: content.implicitHeight

    onRunningChanged: {
        showApp = false
        // Nearby phones show up fast only while this is on screen.
        Pairing.setNearbyShown(running)
        if (running && state_ !== "comparing" && state_ !== "confirmed" && state_ !== "connecting")
            Pairing.start()
        else if (!running)
            stopHosting()
    }
    Component.onCompleted: {
        Pairing.setNearbyShown(running)
        if (running && state_ !== "comparing" && state_ !== "confirmed" && state_ !== "connecting")
            Pairing.start()
    }
    Component.onDestruction: {
        Pairing.setNearbyShown(false)
        stopHosting()
    }

    // Left with its code still showing: the PC stops waiting to be paired.
    function stopHosting() {
        if (state_ === "hosting")
            Pairing.cancel()
    }
    function retry() {
        showApp = false
        Pairing.start()
    }

    // Leave the success message up briefly, then close.
    Timer {
        interval: 1400
        running: panel.state_ === "paired" && panel.running
        onTriggered: panel.finished()
    }

    // Seconds left on the current code.
    property int secondsLeft: 0
    Timer {
        interval: 1000
        repeat: true
        triggeredOnStart: true
        running: panel.running && panel.state_ === "hosting"
        onTriggered: panel.secondsLeft = Math.max(0, Math.round((Pairing.expiresAt - Date.now()) / 1000))
    }

    // Enter pairs the first phone nearby, or confirms matching codes.
    Shortcut {
        sequences: ["Return", "Enter"]
        enabled: panel.running && panel.visible
                 && (panel.state_ === "comparing" || (panel.hosting && Pairing.nearby.length > 0))
        onActivated: {
            if (panel.state_ === "comparing")
                Pairing.confirm(true)
            else
                Pairing.pairNearby(Pairing.nearby[0].id)
        }
    }

    Column {
        id: content
        width: parent.width
        spacing: 0

        Column {
            id: checkBlock
            visible: panel.checking
            width: parent.width
            padding: 20
            spacing: 14
            Txt {
                width: checkBlock.width - 2 * checkBlock.padding
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
                weight: 500
                text: qsTr("Same code on %1?").arg(Pairing.peerName.length > 0 ? Pairing.peerName : qsTr("the phone"))
            }
            CodeDigits {
                code: Pairing.code
            }
            Row {
                visible: panel.state_ === "comparing"
                spacing: 8
                Button { text: qsTr("Codes match"); onClicked: Pairing.confirm(true) }
                Button { variant: "outline"; text: qsTr("They don't match"); onClicked: Pairing.confirm(false) }
            }
            Row {
                visible: panel.state_ === "confirmed"
                height: 34
                spacing: 10
                Spinner { width: 16; height: 16; anchors.verticalCenter: parent.verticalCenter }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Waiting for %1…").arg(Pairing.peerName)
                    muted: true
                }
            }
        }

        Flow {
            id: sides
            visible: !panel.checking
            width: parent.width
            spacing: 0

            // ---- Scan ----
            Item {
                id: scanSide
                width: panel.wide ? Math.floor(sides.width * 0.46) : sides.width
                height: scanCol.implicitHeight + (panel.compact ? 24 : 40)

                Column {
                    id: scanCol
                    anchors.centerIn: parent
                    width: parent.width - 32
                    spacing: 14

                    Item {
                        anchors.horizontalCenter: parent.horizontalCenter
                        readonly property int side: panel.compact ? 164 : 200
                        width: side; height: side
                        QrCode {
                            anchors.fill: parent
                            visible: panel.showApp ? AppController.apkQrPath.length > 0 : panel.state_ === "hosting"
                            path: panel.showApp ? AppController.apkQrPath : Pairing.qrPath
                            modules: panel.showApp ? AppController.apkQrSize : Pairing.qrSize
                            Accessible.role: Accessible.Graphic
                            Accessible.name: panel.showApp ? qsTr("QR code for the Android app") : qsTr("Pairing QR code")
                        }
                        Spinner {
                            anchors.centerIn: parent
                            visible: !panel.showApp && (panel.state_ === "starting" || panel.state_ === "idle"
                                                        || panel.state_ === "connecting")
                            width: 28; height: 28
                        }
                        Icon {
                            anchors.centerIn: parent
                            visible: !panel.showApp && (panel.state_ === "paired" || panel.state_ === "failed")
                            width: 40; height: 40
                            path: panel.state_ === "paired" ? Icons.check : Icons.warning
                            color: panel.state_ === "paired" ? Theme.tintTeal : Theme.tintRed
                        }
                    }
                    Column {
                        width: parent.width
                        spacing: 4
                        Txt {
                            width: parent.width
                            horizontalAlignment: Text.AlignHCenter
                            weight: 500
                            wrapMode: Text.WordWrap
                            elide: Text.ElideNone
                            text: panel.showApp ? qsTr("Scan to get G7 Link for Android")
                                : panel.state_ === "paired" ? qsTr("Paired")
                                : panel.state_ === "failed" ? qsTr("Pairing didn't work")
                                : qsTr("Scan with G7 Link on your phone")
                        }
                        Txt {
                            width: parent.width
                            horizontalAlignment: Text.AlignHCenter
                            visible: !panel.showApp && panel.state_ === "hosting"
                            role: "bodySmall"
                            muted: true
                            text: qsTr("Code changes in %1:%2")
                                  .arg(Math.floor(panel.secondsLeft / 60)).arg(("0" + panel.secondsLeft % 60).slice(-2))
                        }
                        // This PC's code: the phone shows it too before pairing.
                        Txt {
                            width: parent.width
                            horizontalAlignment: Text.AlignHCenter
                            visible: !panel.showApp && panel.state_ === "hosting" && Pairing.linkCode.length === 6
                            weight: 500
                            text: qsTr("Code: %1").arg(Pairing.linkCode.slice(0, 3) + " " + Pairing.linkCode.slice(3))
                        }
                        Row {
                            visible: panel.showApp
                            anchors.horizontalCenter: parent.horizontalCenter
                            spacing: 4
                            TextAction {
                                text: qsTr("Open release page")
                                onClicked: Qt.openUrlExternally(panel.releaseUrl)
                            }
                            TextAction {
                                id: copyLink
                                property bool copied: false
                                text: copied ? qsTr("Copied") : qsTr("Copy link")
                                onClicked: {
                                    AppController.copyText(panel.releaseUrl)
                                    copied = true
                                    copiedTimer.restart()
                                }
                                Timer {
                                    id: copiedTimer
                                    interval: 2000
                                    onTriggered: copyLink.copied = false
                                }
                            }
                            TextAction {
                                text: qsTr("Back to pairing")
                                onClicked: panel.showApp = false
                            }
                        }
                    }
                }
            }

            // ---- Nearby, then the code ----
            Item {
                width: panel.wide ? sides.width - scanSide.width : sides.width
                height: Math.max(panel.wide ? scanSide.height : 0, nearCol.implicitHeight + 32)

                Rectangle {
                    visible: panel.wide
                    width: 1
                    height: parent.height
                    color: Theme.line
                }
                Rectangle {
                    visible: !panel.wide
                    width: parent.width
                    height: 1
                    color: Theme.line
                }

                Column {
                    id: nearCol
                    x: 16
                    y: 16
                    width: parent.width - 32
                    spacing: 2

                    // Looking for phones running "Pair nearby".
                    Item {
                        visible: panel.hosting
                        width: parent.width
                        height: 30
                        Txt {
                            id: nearbyLabel
                            x: Theme.rowPadding
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Nearby")
                            role: "label"
                            muted: true
                        }
                        Rectangle {
                            id: liveDot
                            anchors.left: nearbyLabel.right
                            anchors.leftMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            width: 6; height: 6; radius: 3
                            color: Theme.tintTeal
                            SequentialAnimation on opacity {
                                running: panel.running && panel.hosting && !Theme.reduceMotion
                                loops: Animation.Infinite
                                NumberAnimation { from: 1; to: 0.3; duration: 900; easing.type: Easing.InOutSine }
                                NumberAnimation { from: 0.3; to: 1; duration: 900; easing.type: Easing.InOutSine }
                            }
                        }
                        Txt {
                            anchors.left: liveDot.right
                            anchors.leftMargin: 8
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Looking on this network")
                            role: "bodySmall"
                            muted: true
                        }
                    }
                    Repeater {
                        model: panel.hosting ? Pairing.nearby : []
                        delegate: ActionRow {
                            id: nearbyRow
                            required property var modelData
                            required property int index
                            width: nearCol.width
                            iconPath: Icons.phone
                            tint: index === 0 ? Theme.accent : Theme.tintBlue
                            title: modelData.name
                            selected: index === 0
                            onClicked: Pairing.pairNearby(modelData.id)
                            Button {
                                size: "sm"
                                variant: nearbyRow.index === 0 ? "fill" : "outline"
                                text: qsTr("Pair")
                                onClicked: Pairing.pairNearby(nearbyRow.modelData.id)
                            }
                        }
                    }
                    Txt {
                        visible: panel.hosting && Pairing.nearby.length === 0
                        x: Theme.rowPadding
                        width: parent.width - 2 * Theme.rowPadding
                        height: implicitHeight + 8
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                        role: "bodySmall"
                        muted: true
                        text: qsTr("On the phone, open G7 Link and choose Pair nearby.")
                    }

                    // Connecting to the phone.
                    Row {
                        visible: panel.state_ === "connecting"
                        x: Theme.rowPadding
                        height: Theme.rowHeight
                        spacing: 10
                        Spinner { width: 16; height: 16; anchors.verticalCenter: parent.verticalCenter }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            text: panel.state_ === "connecting"
                                  ? qsTr("Connecting to %1…").arg(Pairing.peerName)
                                  : qsTr("Waiting for %1…").arg(Pairing.peerName)
                        }
                    }

                    // ---- The code ----
                    Item { width: 1; height: 10 }
                    Rectangle {
                        width: parent.width
                        height: 1
                        color: Theme.line
                        visible: panel.state_ !== "paired" && panel.state_ !== "failed"
                    }
                    Item { width: 1; height: 12 }
                    Txt {
                        visible: panel.state_ !== "paired" && panel.state_ !== "failed"
                        x: Theme.rowPadding
                        width: parent.width - 2 * Theme.rowPadding
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                        role: "bodySmall"
                        muted: true
                        text: qsTr("Next you'll compare a 6-digit code on both screens")
                    }
                    Item { width: 1; height: 8 }
                    CodeDigits {
                        visible: panel.state_ !== "paired" && panel.state_ !== "failed"
                        x: Theme.rowPadding
                        code: "······"
                        dim: true
                    }

                    // ---- Outcome ----
                    Txt {
                        visible: panel.state_ === "paired" || panel.state_ === "failed"
                        x: Theme.rowPadding
                        width: parent.width - 2 * Theme.rowPadding
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                        muted: panel.state_ === "failed"
                        text: {
                            if (panel.state_ === "paired")
                                return qsTr("%1 is paired with this PC.").arg(Pairing.peerName)
                            switch (Pairing.failure) {
                            // The other device's "They don't match" and Cancel look the same from here.
                            case "rejected": return qsTr("Cancelled, or the codes didn't match. Nothing was paired.")
                            case "declined": return qsTr("Declined on one of the devices.")
                            case "unreachable": return qsTr("Couldn't reach the phone. Put both on the same network.")
                            case "expired": return qsTr("The code expired.")
                            default: return qsTr("Something went wrong.")
                            }
                        }
                    }
                    Item { width: 1; height: 8; visible: panel.state_ === "failed" }
                    Button {
                        visible: panel.state_ === "failed"
                        x: Theme.rowPadding
                        text: qsTr("Try again")
                        iconPath: Icons.refresh
                        onClicked: panel.retry()
                    }
                }
            }
        }

        // ---- Bottom line ----
        Rectangle {
            width: parent.width
            height: 1
            color: Theme.line
        }
        Item {
            width: parent.width
            height: 40
            Row {
                x: 14
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: panel.width >= 420
                    text: qsTr("No G7 Link on the phone yet?")
                    role: "bodySmall"
                    muted: true
                }
                TextAction {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Get it for Android")
                    muted: false
                    selected: panel.showApp
                    onClicked: panel.showApp = !panel.showApp
                }
            }
            TextAction {
                anchors.right: parent.right
                anchors.rightMargin: 14
                anchors.verticalCenter: parent.verticalCenter
                visible: panel.cancellable && panel.state_ !== "paired"
                text: qsTr("Cancel")
                onClicked: panel.cancelled()
            }
        }
    }
}
