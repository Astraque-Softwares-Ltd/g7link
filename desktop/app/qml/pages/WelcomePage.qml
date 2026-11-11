// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// While G7 Link starts, and the (rare) case where it couldn't. Looks like
// the welcome screen: the G7 mark, then what's happening.
Item {
    id: page

    readonly property string status: AppController.status

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
    }

    Column {
        anchors.centerIn: parent
        width: Math.min(520, parent.width - 32)
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
                    text: page.status === "failed" ? qsTr("Couldn't start") : qsTr("Starting")
                    color: page.status === "failed" ? Theme.tintRed : Theme.surfaceContentVariant
                }
            }
            Spinner {
                anchors.verticalCenter: parent.verticalCenter
                visible: page.status !== "failed"
                width: 18; height: 18
            }
        }

        Column {
            width: parent.width
            spacing: 12
            visible: page.status === "failed"
            Txt {
                width: parent.width
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
                role: "code"
                size: 12
                muted: true
                text: AppController.error
            }
            Button {
                variant: "outline"
                iconPath: Icons.folder
                text: qsTr("Open the logs folder")
                onClicked: Qt.openUrlExternally(AppController.logsUrl())
            }
        }
    }

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
        Txt {
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Everything stays on your network. No account.")
            role: "bodySmall"
            muted: true
        }
    }
}
