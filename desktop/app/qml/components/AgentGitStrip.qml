// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The source-control strip in a thread's header (§SC9.1): the branch (or
// where HEAD is detached; a worktree's folder glyph before it), what
// changed, commits to push and on the remote (as of the last fetch), and
// the branch's pull request with its checks. Any part opens the panel.
Row {
    id: strip
    // Agents.git's strip: { repo, branch, detached, worktree, changed, ahead, behind, pr }.
    property var info: ({})
    // The panel is open.
    property bool open: false
    signal clicked

    readonly property bool repo: info.repo === "ok"
    readonly property int changed: info.changed || 0
    readonly property int ahead: info.ahead || 0
    readonly property int behind: info.behind || 0
    readonly property var pr: info.pr || ({})
    readonly property bool hasPr: (pr.number || 0) > 0
    readonly property string where: (info.branch || "").length > 0 ? info.branch
        : qsTr("Detached at %1").arg(info.detached || "")
    readonly property color prTone: !hasPr ? Theme.surfaceContentVariant
        : pr.state === "merged" ? AgentPalette.done
        : (pr.fail || 0) > 0 ? AgentPalette.failed
        : (pr.pending || 0) > 0 ? AgentPalette.working
        : (pr.pass || 0) > 0 ? AgentPalette.done : Theme.surfaceContentVariant
    readonly property string prWords: !hasPr ? ""
        : (pr.fail || 0) > 0 ? qsTr("%1 of %2 failed").arg(pr.fail).arg((pr.pass || 0) + pr.fail + (pr.pending || 0))
        : (pr.pending || 0) > 0 ? qsTr("%1 of %2 running").arg(pr.pending).arg((pr.pass || 0) + (pr.pending || 0))
        : (pr.pass || 0) > 0 ? qsTr("%n checks passed", "", pr.pass) : qsTr("No checks")

    visible: repo
    spacing: 2

    Rectangle {
        id: main
        width: mainRow.implicitWidth + 20
        height: 28
        radius: Theme.radiusSm
        color: strip.open ? Theme.activeFill : mainHover.hovered ? Theme.selection : Theme.raised
        border.width: main.activeFocus ? 1 : 0
        border.color: Theme.focusRing
        activeFocusOnTab: true
        Accessible.role: Accessible.Button
        Accessible.name: qsTr("Source control")
        Accessible.description: [strip.where, strip.changed > 0 ? qsTr("%n changed", "", strip.changed) : ""]
            .filter((s) => s.length > 0).join(", ")
        Accessible.onPressAction: strip.clicked()
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                strip.clicked()
                event.accepted = true
            }
        }
        Row {
            id: mainRow
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                width: 14
                height: 14
                path: strip.info.worktree === true ? Icons.folder : AgentGitGlyphs.branch
                color: Theme.surfaceContentVariant
            }
            Txt {
                anchors.verticalCenter: parent.verticalCenter
                width: Math.min(implicitWidth, 220)
                text: strip.where
                role: "code"
                size: Theme.typography.bodySmall.size
                maximumLineCount: 1
            }
            Txt {
                visible: strip.changed > 0
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("%n changed", "", strip.changed)
                role: "bodySmall"
                muted: true
            }
        }
        HoverHandler {
            id: mainHover
            cursorShape: Qt.PointingHandCursor
        }
        TapHandler {
            onTapped: strip.clicked()
        }
    }
    IconButton {
        visible: strip.ahead > 0
        anchors.verticalCenter: parent.verticalCenter
        iconPath: AgentGitGlyphs.up
        iconSize: 12
        text: String(strip.ahead)
        label: qsTr("%n commits to push", "", strip.ahead)
        onClicked: strip.clicked()
    }
    IconButton {
        visible: strip.behind > 0
        anchors.verticalCenter: parent.verticalCenter
        iconPath: AgentGitGlyphs.down
        iconSize: 12
        text: String(strip.behind)
        label: qsTr("%n commits on the remote you don't have, as of the last fetch", "", strip.behind)
        onClicked: strip.clicked()
    }
    IconButton {
        visible: strip.hasPr
        anchors.verticalCenter: parent.verticalCenter
        iconPath: strip.pr.state === "merged" ? AgentGitGlyphs.merged : AgentGitGlyphs.pullRequest
        iconSize: 12
        iconColor: strip.prTone
        text: "#" + (strip.pr.number || 0)
        label: strip.prWords
        onClicked: strip.clicked()
    }
}
