// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Remove a worktree (§SC9.4): its folder is deleted and its branch stays,
// with the commits no remote has counted. With changes in it, it's refused
// before anything runs. Cancel has the keyboard; Remove takes a click, or
// Tab then Enter.
Sheet {
    id: dialog
    // A worktree's id ("" for the open thread's), branch, its commits on no
    // remote, and its uncommitted files (refused when there are any).
    property string worktreeId
    property string branch
    property int unpushed: 0
    property int dirty: 0

    function ask(id, branchName, unpushedCount, dirtyFiles) {
        dialog.worktreeId = id
        dialog.branch = branchName
        dialog.unpushed = unpushedCount
        dialog.dirty = dirtyFiles
        open()
        Qt.callLater(() => cancelButton.forceActiveFocus(Qt.OtherFocusReason))
    }

    cardWidth: 460

    Column {
        width: parent.width
        spacing: 14
        Txt {
            width: parent.width
            text: qsTr("Remove this worktree?")
            role: "title"
        }
        Txt {
            visible: dialog.dirty === 0
            width: parent.width
            text: qsTr("The folder is deleted. The branch %1 stays in your repository.").arg(dialog.branch)
            role: "body"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Txt {
            visible: dialog.dirty === 0 && dialog.unpushed > 0
            width: parent.width
            text: qsTr("%n of its commits aren't on any remote. They stay on the branch.", "", dialog.unpushed)
            role: "body"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Txt {
            visible: dialog.dirty > 0
            width: parent.width
            text: qsTr("This worktree has uncommitted changes. Commit or delete them first.")
            role: "body"
            color: AgentPalette.failedText
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                id: cancelButton
                variant: "text"
                text: dialog.dirty > 0 ? qsTr("Close") : qsTr("Cancel")
                onClicked: dialog.close()
            }
            Button {
                visible: dialog.dirty === 0
                variant: "tonal"
                iconPath: Icons.trash
                text: qsTr("Remove")
                onClicked: {
                    if (dialog.worktreeId.length > 0)
                        Agents.worktreeRemove(dialog.worktreeId, false)
                    else
                        Agents.worktreeRemoveOpen()
                    dialog.close()
                }
            }
        }
    }
}
