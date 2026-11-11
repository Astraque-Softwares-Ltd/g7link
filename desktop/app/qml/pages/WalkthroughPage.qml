// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// First run, on one screen: the G7 mark, what this PC is ready for (each
// with its fix), and Pair a phone or Later. Reopened from Settings › About.
FocusScope {
    id: welcome
    focus: true

    property bool active: true
    property string deviceId: ""
    property string deviceName: ""

    signal completed()
    signal openGuideRequested()

    readonly property var doctorChecks: { try { return JSON.parse(AppController.doctorChecks || "[]") } catch (e) { return [] } }
    function doctorCheck(id) { return doctorChecks.find((c) => c.id === id) || null }
    readonly property var firewallCheck: doctorCheck("firewall")
    readonly property var webcamCheck: doctorCheck("linux-v4l2loopback")
    readonly property var inputCheck: doctorCheck("linux-remote-input")

    // { label, icon, ok, state, fix (label, empty when there's nothing to do), run }
    readonly property var checks: [
        {
            label: qsTr("Local network"),
            icon: Icons.wifi,
            ok: firewallCheck !== null && firewallCheck.outcome === "ok",
            state: firewallCheck === null
                ? (AppController.doctorBusy ? qsTr("Checking") : qsTr("Not checked"))
                : (firewallCheck.outcome === "ok" ? qsTr("Firewall allows G7 Link") : qsTr("Firewall blocks G7 Link")),
            fix: firewallCheck !== null && firewallCheck.outcome !== "ok" ? qsTr("Set up") : "",
            run: () => {
                if (welcome.firewallCheck && welcome.firewallCheck.fix)
                    AppController.doctorFix(welcome.firewallCheck.fix)
                else
                    AppController.runDoctor()
            }
        },
        {
            label: qsTr("Notifications"),
            icon: Icons.bell,
            ok: AppController.toastsEnabled,
            state: AppController.toastsEnabled ? qsTr("On") : qsTr("Off in system settings"),
            fix: AppController.toastsEnabled ? "" : qsTr("Check again"),
            run: () => AppController.refreshToastsEnabled()
        },
        {
            label: qsTr("Phone as webcam"),
            icon: Icons.video,
            ok: Webcam.addonRegistered,
            state: Webcam.addonRegistered ? qsTr("Ready")
                : (Webcam.addonBusy ? qsTr("Setting up") : qsTr("Needs v4l2loopback")),
            fix: Webcam.addonRegistered || Webcam.addonBusy ? "" : qsTr("Set up"),
            run: () => {
                if (welcome.webcamCheck !== null && welcome.webcamCheck.outcome !== "ok")
                    AppController.runDoctor()
                else
                    Webcam.setupAddon()
            }
        },
        {
            label: qsTr("Phone as mouse and keyboard"),
            icon: Icons.keyboard,
            ok: inputCheck !== null && inputCheck.outcome === "ok",
            state: inputCheck === null ? (AppController.doctorBusy ? qsTr("Checking") : qsTr("Not checked"))
                : (inputCheck.outcome === "ok" ? qsTr("Ready") : qsTr("Needs /dev/uinput access")),
            fix: inputCheck !== null && inputCheck.outcome !== "ok" ? qsTr("Set up") : "",
            run: () => AppController.runDoctor()
        }
    ]

    function finish() {
        Preferences.walkthroughCompleted = true
        welcome.completed()
    }
    function pair() {
        pairingSheet.open()
    }

    Component.onCompleted: {
        AppController.refreshToastsEnabled()
        if (doctorChecks.length === 0 && !AppController.doctorBusy)
            AppController.recheckDoctor()
    }
    // Pairing here finishes the welcome.
    Connections {
        target: AppController
        function onHasDevicesChanged() {
            if (AppController.hasDevices && pairingSheet.opened)
                pairedTimer.restart()
        }
    }
    Timer {
        id: pairedTimer
        interval: 1200
        onTriggered: {
            pairingSheet.close()
            welcome.finish()
        }
    }

    Keys.onPressed: (event) => {
        if (pairingSheet.opened)
            return
        if (event.key === Qt.Key_Escape) {
            welcome.finish()
            event.accepted = true
        } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            welcome.pair()
            event.accepted = true
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
    }

    Flickable {
        anchors.fill: parent
        anchors.topMargin: Theme.headerHeight
        anchors.bottomMargin: Theme.footerHeight
        contentHeight: Math.max(height, content.height + 64)
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: content
            width: Math.min(520, parent.width - 32)
            x: Math.round((parent.width - width) / 2)
            y: Math.max(32, Math.round((parent.height - height) / 2))
            spacing: 22

            Row {
                spacing: 14
                Icon {
                    width: 52; height: 52
                    path: Icons.mark
                    color: Theme.accent
                    stroke: 2
                }
                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 2
                    Txt {
                        text: "G7 Link"
                        role: "displayLarge"
                    }
                    Txt {
                        text: qsTr("Your Android phone, on this PC")
                        muted: true
                    }
                }
            }

            Column {
                width: parent.width
                spacing: 2
                SectionLabel {
                    width: parent.width
                    text: qsTr("This PC")
                }
                Repeater {
                    model: welcome.checks
                    delegate: ActionRow {
                        required property var modelData
                        width: parent.width
                        interactive: false
                        iconPath: modelData.ok ? Icons.check : modelData.icon
                        tint: modelData.ok ? Theme.tintTeal : Theme.tintAmber
                        title: modelData.label
                        meta: modelData.state
                        Button {
                            visible: modelData.fix.length > 0
                            size: "sm"
                            variant: "outline"
                            text: modelData.fix
                            enabled: !AppController.doctorBusy
                            onClicked: modelData.run()
                        }
                    }
                }
            }

            Row {
                spacing: 8
                Button {
                    text: qsTr("Pair a phone")
                    onClicked: welcome.pair()
                }
                Button {
                    variant: "text"
                    text: AppController.hasDevices ? qsTr("Done") : qsTr("Later")
                    onClicked: welcome.finish()
                }
            }
        }
    }

    // ---- Footer ----
    Item {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: Theme.footerHeight
        Rectangle {
            width: parent.width
            height: 1
            color: Theme.line
        }
        Row {
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 14
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Everything stays on your network. No account.")
                role: "bodySmall"
                muted: true
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Get the Android app")
                onClicked: Qt.openUrlExternally("https://github.com/Astraque-Softwares-Ltd/g7link/releases/latest")
            }
        }
        Row {
            anchors.right: parent.right
            anchors.rightMargin: 14
            anchors.verticalCenter: parent.verticalCenter
            spacing: 10
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Guide")
                onClicked: {
                    Preferences.walkthroughCompleted = true
                    welcome.openGuideRequested()
                }
            }
            TextAction {
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Pair")
                muted: false
                onClicked: welcome.pair()
            }
            Keycap {
                anchors.verticalCenter: parent.verticalCenter
                keys: "Enter"
            }
        }
    }

    Sheet {
        id: pairingSheet
        cardWidth: 480
        onOpenedChanged: {
            if (!opened) {
                Pairing.cancel()
                welcome.forceActiveFocus()
            }
        }
        Loader {
            id: pairingLoader
            width: parent.width
            active: pairingSheet.opened
            source: active ? "qrc:/qt/qml/app/g7link/qml/pages/PairingPanel.qml" : ""
            onLoaded: {
                item.width = Qt.binding(() => pairingLoader.width)
                item.running = Qt.binding(() => pairingSheet.opened)
                item.finished.connect(() => {
                    pairingSheet.close()
                    if (AppController.hasDevices)
                        welcome.finish()
                })
                item.cancelled.connect(() => pairingSheet.close())
            }
        }
    }
}
