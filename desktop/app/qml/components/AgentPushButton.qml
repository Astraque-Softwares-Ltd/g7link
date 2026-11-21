// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Push (§SC9.2): one button with the commits it sends; on the repository's
// default branch it has a menu, "Push to <branch>" or "New branch first…".
// When the branch has no upstream and there are several remotes, the host
// asks which: the remotes are listed to choose from.
Column {
    id: push
    property int ahead: 0
    property string branch
    property bool onDefault: false
    // Remotes to choose from ([] unless the host asked).
    property var remotes: []
    property bool busy: false
    signal pushed(string remote)
    signal newBranchFirst
    signal menuAt(real x, real y, var actions)

    spacing: 6

    Row {
        spacing: 2
        Button {
            id: pushButton
            size: "sm"
            variant: "tonal"
            iconPath: AgentGitGlyphs.up
            text: push.ahead > 0 ? qsTr("Push") + "  ↑" + push.ahead : qsTr("Push")
            enabled: !push.busy
            onClicked: push.pushed("")
        }
        IconButton {
            visible: push.onDefault
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.chevronDown
            label: qsTr("More push choices")
            enabled: !push.busy
            onClicked: {
                const p = mapToItem(null, 0, height)
                push.menuAt(p.x, p.y, [
                    { label: qsTr("Push to %1").arg(push.branch), icon: AgentGitGlyphs.up, run: () => push.pushed("") },
                    { label: qsTr("New branch first…"), icon: AgentGitGlyphs.branch, run: () => push.newBranchFirst() }
                ])
            }
        }
    }
    Column {
        visible: push.remotes.length > 0
        width: parent.width
        spacing: 2
        Txt {
            text: qsTr("Choose a remote")
            role: "label"
            muted: true
        }
        Repeater {
            model: push.remotes
            delegate: ActionRow {
                id: remoteRow
                required property var modelData
                width: push.width
                iconPath: AgentGitGlyphs.up
                title: modelData.name || modelData
                detail: modelData.label || ""
                onClicked: push.pushed(remoteRow.modelData.name || remoteRow.modelData)
            }
        }
    }
}
