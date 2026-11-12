// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Relay servers: the ones this PC may use away from home, the ones in use
// now, and how each answers a check.
Sheet {
    id: sheet
    cardWidth: 540

    property string snapshotJson: "null"
    property string editError: ""
    readonly property var snapshot: { try { return JSON.parse(snapshotJson) } catch (e) { return null } }
    readonly property var probes: { try { return JSON.parse(AppController.relayProbes || "[]") } catch (e) { return [] } }

    function regionTitle(key) {
        switch (key) {
        case "north_america_east": return qsTr("North America · East")
        case "north_america_west": return qsTr("North America · West")
        case "europe": return qsTr("Europe")
        case "asia_pacific": return qsTr("Asia-Pacific")
        default: return qsTr("Custom")
        }
    }

    function probeText(url) {
        if (AppController.relayCheckBusy)
            return ""
        const probe = sheet.probes.find(p => p.url === url)
        if (!probe)
            return ""
        if (probe.rtt_ms !== null)
            return qsTr("%1 ms").arg(probe.rtt_ms)
        return probe.timed_out ? qsTr("Timed out") : qsTr("Unreachable")
    }

    function edit(url, action) {
        switch (AppController.editRelay(url, action)) {
        case "":
            editError = ""
            snapshotJson = AppController.relaySnapshot()
            return true
        case "invalid":
            editError = qsTr("Enter an HTTP or HTTPS address without a login, path or query.")
            break
        case "duplicate":
            editError = qsTr("Already in the list.")
            break
        case "limit":
            editError = qsTr("Up to 16 custom relays.")
            break
        case "unavailable":
            editError = qsTr("G7 Link isn't running.")
            break
        default:
            editError = qsTr("Couldn't save.")
        }
        return false
    }

    function addRelay() {
        if (address.text.trim().length > 0 && edit(address.text, "add"))
            address.text = ""
    }

    onOpenedChanged: {
        if (opened) {
            snapshotJson = AppController.relaySnapshot()
            editError = ""
        }
    }

    Timer {
        interval: 2000
        repeat: true
        running: sheet.opened
        onTriggered: sheet.snapshotJson = AppController.relaySnapshot()
    }

    Column {
        width: parent.width
        spacing: 4

        Item {
            width: parent.width
            height: Math.max(heading.height, busy.height)
            Txt {
                id: heading
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Relay servers")
                role: "title"
            }
            Spinner {
                id: busy
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                visible: AppController.relayCheckBusy
            }
        }
        Txt {
            width: parent.width
            visible: sheet.snapshot === null || !Preferences.awayMode
            wrapMode: Text.WordWrap
            role: "bodySmall"
            muted: true
            text: sheet.snapshot === null ? qsTr("Starting…")
                                          : qsTr("Used when Connect over mobile data is on.")
        }

        SectionLabel { width: parent.width; text: qsTr("Home relay") }
        ActionRow {
            width: parent.width
            visible: !sheet.snapshot || sheet.snapshot.home.length === 0
            interactive: false
            title: qsTr("None")
            titleColor: Theme.surfaceContentVariant
        }
        Repeater {
            model: sheet.snapshot ? sheet.snapshot.home : []
            delegate: ActionRow {
                required property var modelData
                width: parent.width
                interactive: false
                title: modelData.url
                meta: modelData.connected ? qsTr("Connected")
                    : modelData.failed ? qsTr("Can't connect") : qsTr("Connecting…")
            }
        }

        SectionLabel { width: parent.width; text: qsTr("Devices via relay") }
        ActionRow {
            width: parent.width
            visible: !sheet.snapshot || sheet.snapshot.routes.length === 0
            interactive: false
            title: qsTr("None")
            titleColor: Theme.surfaceContentVariant
        }
        Repeater {
            model: sheet.snapshot ? sheet.snapshot.routes : []
            delegate: ActionRow {
                required property var modelData
                width: parent.width
                interactive: false
                title: modelData.device
                detail: modelData.url
            }
        }

        SectionLabel { width: parent.width; text: qsTr("Servers") }
        Repeater {
            model: sheet.snapshot ? sheet.snapshot.servers : []
            delegate: ActionRow {
                id: server
                required property var modelData
                width: parent.width
                title: modelData.url
                detail: modelData.provider.length > 0
                    ? qsTr("%1 · %2").arg(modelData.provider).arg(sheet.regionTitle(modelData.region))
                    : sheet.regionTitle(modelData.region)
                meta: sheet.probeText(modelData.url)
                onClicked: sheet.edit(server.modelData.url, server.modelData.enabled ? "disable" : "enable")
                IconButton {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: server.modelData.region === "custom"
                    iconPath: Icons.trash
                    iconColor: Theme.tintRed
                    label: qsTr("Remove")
                    onClicked: sheet.edit(server.modelData.url, "remove")
                }
                Toggle {
                    anchors.verticalCenter: parent.verticalCenter
                    label: server.modelData.url
                    checked: server.modelData.enabled
                    onToggled: (on) => sheet.edit(server.modelData.url, on ? "enable" : "disable")
                }
            }
        }

        Item { width: 1; height: 4 }
        Row {
            width: parent.width
            spacing: 8
            Field {
                id: address
                width: parent.width - addButton.width - parent.spacing
                anchors.verticalCenter: parent.verticalCenter
                placeholder: "https://relay.example.com"
                enabled: sheet.snapshot !== null
                Accessible.name: qsTr("Relay address")
                onAccepted: sheet.addRelay()
            }
            Button {
                id: addButton
                anchors.verticalCenter: parent.verticalCenter
                variant: "tonal"
                size: "sm"
                iconPath: Icons.plus
                text: qsTr("Add")
                enabled: sheet.snapshot !== null && address.text.trim().length > 0
                onClicked: sheet.addRelay()
            }
        }
        Txt {
            width: parent.width
            visible: text.length > 0
            wrapMode: Text.WordWrap
            role: "bodySmall"
            color: Theme.error
            text: sheet.editError
        }

        Item { width: 1; height: 4 }
        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                variant: "text"
                text: qsTr("Check all")
                enabled: sheet.snapshot !== null && !AppController.relayCheckBusy
                onClicked: AppController.checkRelays()
            }
            Button { text: qsTr("Done"); onClicked: sheet.close() }
        }
    }
}
