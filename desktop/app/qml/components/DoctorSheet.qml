// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// The Connection Doctor: what stands between the phone and this PC, worst
// first, each with a fix when one can be made from here.
Sheet {
    id: sheet
    cardWidth: 520

    readonly property var checks: { try { return JSON.parse(AppController.doctorChecks) } catch (e) { return [] } }
    readonly property bool allWell: checks.length > 0 && checks.every(c => c.outcome === "ok")
    // Checks for features that need setup (ids "linux-..."), as opposed to
    // what stands between the phone and this PC.
    readonly property bool connectionWell: checks.length > 0
        && checks.every(c => c.outcome === "ok" || c.id.startsWith("linux-"))

    function fixLabel(fix) {
        switch (fix) {
        case "firewall": return qsTr("Allow")
        case "network-settings": return qsTr("Open settings")
        case "reconnect": return qsTr("Reconnect")
        default: return ""
        }
    }

    Column {
        width: parent.width
        spacing: Theme.quiet ? 10 : 16

        Item {
            width: parent.width
            height: Math.max(heading.height, busy.height)
            Txt {
                id: heading
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Connection Doctor")
                role: Theme.quiet ? "title" : "headline"
            }
            Spinner {
                id: busy
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                visible: AppController.doctorBusy
            }
        }
        Txt {
            width: parent.width
            wrapMode: Text.WordWrap
            role: Theme.quiet ? "bodySmall" : "body"
            muted: true
            text: AppController.doctorBusy && sheet.checks.length === 0
                  ? qsTr("Checking network and firewall…")
                  : sheet.allWell
                    ? qsTr("Everything looks right.")
                    : sheet.connectionWell
                      ? qsTr("Your phone reaches this PC. A few features need a step.")
                      : qsTr("Your phone can't reach this PC.")
        }

        Flickable {
            width: parent.width
            height: Math.min(checksColumn.height, 360)
            contentWidth: width
            contentHeight: checksColumn.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: checksColumn
                width: parent.width
                spacing: 2
                opacity: AppController.doctorBusy ? 0.55 : 1
                Behavior on opacity { NumberAnimation { duration: Theme.fadeNormal } }
                Repeater {
                    model: sheet.checks
                    delegate: Rectangle {
                        id: row
                        required property var modelData
                        readonly property bool hasFix: row.modelData.fix !== undefined && row.modelData.fix !== ""
                        width: parent.width
                        height: Theme.quiet
                            ? Math.max(Theme.rowHeight, words.height + 16, actionsRow.height + 8)
                            : Math.max(badge.height, words.height, actionsRow.height) + 20
                        radius: Theme.radiusMd
                        color: "transparent"

                        Rectangle {
                            id: badge
                            x: Theme.quiet ? Theme.rowPadding : 8
                            // On the title's first line, however many lines it wraps to.
                            y: words.y + Math.round((checkTitle.lineHeight - height) / 2)
                            width: Theme.quiet ? 18 : 28; height: width
                            radius: width / 2
                            color: Theme.quiet ? "transparent" : row.modelData.outcome === "ok" ? Theme.secondaryContainer
                                 : row.modelData.outcome === "warn" ? Qt.alpha(Theme.warning, 0.18)
                                 : Qt.alpha(Theme.error, 0.16)
                            Icon {
                                anchors.centerIn: parent
                                width: Theme.quiet ? 18 : 16; height: width
                                path: row.modelData.outcome === "ok" ? Icons.check : Icons.warning
                                color: Theme.quiet
                                    ? (row.modelData.outcome === "ok" ? Theme.tintTeal
                                       : row.modelData.outcome === "warn" ? Theme.tintAmber : Theme.tintRed)
                                    : (row.modelData.outcome === "ok" ? Theme.secondaryContainerContent
                                       : row.modelData.outcome === "warn" ? Theme.warning : Theme.error)
                            }
                        }
                        Column {
                            id: words
                            anchors.left: badge.right
                            anchors.leftMargin: Theme.quiet ? 12 : 14
                            anchors.right: actionsRow.visible ? actionsRow.left : parent.right
                            anchors.rightMargin: actionsRow.visible ? 12 : 8
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 3
                            Txt {
                                id: checkTitle
                                width: parent.width
                                text: row.modelData.title
                                role: Theme.quiet ? "body" : "title"
                                size: Theme.quiet ? Theme.typography.body.size : 14
                                wrapMode: Text.WordWrap
                                elide: Text.ElideNone
                            }
                            Txt {
                                width: parent.width
                                visible: row.modelData.detail.length > 0
                                text: row.modelData.detail
                                role: "bodySmall"
                                muted: true
                                wrapMode: Text.WordWrap
                            }
                        }
                        Row {
                            id: actionsRow
                            anchors.right: parent.right
                            anchors.rightMargin: 6
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 6
                            visible: row.hasFix

                            Button {
                                visible: row.modelData.fix === "firewall"
                                enabled: !AppController.doctorBusy
                                variant: "tonal"
                                size: "sm"
                                text: qsTr("Firewall settings")
                                onClicked: AppController.doctorFix("firewall-settings")
                            }
                            Button {
                                id: fixButton
                                // A fix this sheet has no words for gets no blank button.
                                visible: sheet.fixLabel(row.modelData.fix).length > 0
                                enabled: !AppController.doctorBusy
                                variant: row.modelData.outcome === "fail" ? "fill" : "tonal"
                                size: "sm"
                                text: sheet.fixLabel(row.modelData.fix)
                                onClicked: AppController.doctorFix(row.modelData.fix)
                            }
                        }
                    }
                }
            }
        }

        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                variant: "text"
                text: qsTr("Check again")
                enabled: !AppController.doctorBusy
                onClicked: AppController.runDoctor()
            }
            Button { text: qsTr("Done"); onClicked: sheet.close() }
        }
    }
}
