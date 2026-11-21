// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A job's output (§SC9.1): git's and your hooks' lines, plain, in mono on
// the code ground, the last 12 in view and the rest a scroll away; "N
// earlier lines not kept"; Stop while a commit or a push runs; then the
// last line in words.
Column {
    id: output
    // Agents.git's job: { kind, phase, running, lines, dropped, words, args, reason, stoppable }.
    property var job: ({})
    property string agentName
    signal stopped

    readonly property var lines: job.lines || []
    readonly property bool failed: job.phase === "failed" || job.phase === "stopped"
    readonly property string last: {
        const a = job.args || []
        switch (job.words) {
        case "running_hooks": return qsTr("Running your hooks…")
        case "committed": return qsTr("Committed %1 · %2").arg(a[0] || "").arg(a[1] || "")
        case "commit_hook": return qsTr("Your commit hook stopped the commit.")
        case "commit_failed": return qsTr("The commit didn't go through.")
        case "commit_stopped": return qsTr("The commit was stopped. Some files may be left staged.")
        case "pushing": return qsTr("Pushing to %1…").arg(a[0] || "")
        case "pushed": return qsTr("Pushed %1 to %2").arg(a[0] || "").arg(a[1] || "")
        case "up_to_date": return qsTr("Already up to date")
        case "push_behind": return qsTr("The remote has commits you don't have. Pull in a terminal, or ask %1 in this thread, then push again.").arg(output.agentName)
        case "push_remote_refused": return qsTr("The remote refused the push.")
        case "push_hook": return qsTr("Your pre-push hook stopped the push.")
        case "push_sign_in": return qsTr("Git couldn't sign in to the remote. Push once from a terminal to set it up.")
        case "push_network": return qsTr("The remote couldn't be reached.")
        case "push_failed": return qsTr("The push didn't go through.")
        case "switched": return qsTr("Switched to %1").arg(a[0] || "")
        case "created": return qsTr("Created %1").arg(a[0] || "")
        case "would_overwrite": return qsTr("Git didn't switch: these files would be overwritten. Commit them first.")
        case "switch_failed": return qsTr("Git didn't switch.")
        case "opening": return qsTr("Opening…")
        case "pr_opened": return qsTr("Opened pull request #%1").arg(a[0] || "")
        case "pr_failed": return qsTr("GitHub didn't open it.")
        case "merged": return qsTr("Merged pull request #%1").arg(a[0] || "")
        case "merge_failed": return qsTr("GitHub didn't merge it.")
        case "creating_worktree": return qsTr("Creating the worktree…")
        case "worktree_failed": return qsTr("The worktree couldn't be created.")
        default: return ""
        }
    }

    visible: lines.length > 0 || last.length > 0
    spacing: 6

    Rectangle {
        visible: output.lines.length > 0 || (output.job.dropped || 0) > 0
        width: parent.width
        height: Math.min(lineList.contentHeight, 12 * 17) + 16
        radius: Theme.radiusSm
        color: AgentPalette.codeGround
        ListView {
            id: lineList
            anchors.fill: parent
            anchors.margins: 8
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: output.lines
            // The newest line stays in view as lines arrive.
            onCountChanged: Qt.callLater(() => lineList.positionViewAtEnd())
            header: Txt {
                visible: (output.job.dropped || 0) > 0
                height: visible ? 17 : 0
                text: qsTr("%n earlier lines not kept", "", output.job.dropped || 0)
                role: "caption"
                muted: true
            }
            delegate: Text {
                required property string modelData
                width: lineList.width
                text: modelData
                textFormat: Text.PlainText
                wrapMode: Text.NoWrap
                elide: Text.ElideRight
                color: AgentPalette.diffText
                font.family: Theme.fontMono
                font.features: Theme.monoFeatures
                font.pixelSize: Theme.typography.caption.size
                height: 17
                verticalAlignment: Text.AlignVCenter
            }
        }
    }
    Item {
        visible: output.last.length > 0 || output.job.stoppable === true
        width: parent.width
        height: Math.max(lastLine.implicitHeight, stopButton.visible ? stopButton.height : 0)
        Row {
            id: lastLine
            anchors.left: parent.left
            anchors.right: stopButton.visible ? stopButton.left : parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 6
            Spinner {
                visible: output.job.running === true
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
            }
            Icon {
                visible: output.job.running !== true && output.last.length > 0
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: output.failed ? AgentGitGlyphs.failed : AgentGitGlyphs.passed
                color: output.failed ? AgentPalette.failed : AgentPalette.done
            }
            Txt {
                width: lastLine.width - 20
                anchors.verticalCenter: parent.verticalCenter
                text: output.last
                role: "bodySmall"
                color: output.failed ? AgentPalette.failedText : Theme.surfaceContent
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }
        }
        Button {
            id: stopButton
            visible: output.job.stoppable === true
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            size: "sm"
            variant: "tonal"
            iconPath: Icons.stopSquare
            text: qsTr("Stop")
            onClicked: output.stopped()
        }
    }
}
