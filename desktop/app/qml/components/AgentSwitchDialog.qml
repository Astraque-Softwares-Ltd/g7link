// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Switching with changes in the folder (§SC9.2): git takes them along if
// they don't clash, and otherwise stops with nothing changed. Cancel has the
// keyboard.
Sheet {
    id: dialog
    property string branch
    property int changed: 0

    function ask(name, files) {
        dialog.branch = name
        dialog.changed = files
        open()
        Qt.callLater(() => cancelButton.forceActiveFocus(Qt.OtherFocusReason))
    }

    cardWidth: 440

    Column {
        width: parent.width
        spacing: 14
        Txt {
            width: parent.width
            text: qsTr("Switch to %1?").arg(dialog.branch)
            role: "title"
            wrapMode: Text.WordWrap
        }
        Txt {
            width: parent.width
            text: qsTr("%n files have changes. Git takes them along if they don't clash; if they do, it stops and nothing changes.", "", dialog.changed)
            role: "body"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                id: cancelButton
                variant: "text"
                text: qsTr("Cancel")
                onClicked: dialog.close()
            }
            Button {
                variant: "tonal"
                text: qsTr("Switch")
                onClicked: {
                    Agents.gitSwitch(dialog.branch, false, true)
                    dialog.close()
                }
            }
        }
    }
}
