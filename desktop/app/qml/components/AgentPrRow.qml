// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A row of the Pull Requests list (§SC4.4): the title, its repository,
// number and draft, its checks, how long ago it moved; "Open thread" when a
// thread here works on its branch, "Open on GitHub", and "Copy link".
Item {
    id: row
    // { number, title, repo, state, draft, updatedMs, url, canOpen, thread, checks: { pass, fail, pending } }
    property var pr: ({})
    property real now: Date.now()
    signal openThread(string id)

    readonly property var checks: pr.checks || ({})
    readonly property int total: (checks.pass || 0) + (checks.fail || 0) + (checks.pending || 0)
    function ago(ms) {
        const s = Math.max(0, (row.now - ms) / 1000)
        if (s < 3600)
            return qsTr("%1 min").arg(Math.max(1, Math.floor(s / 60)))
        if (s < 86400)
            return qsTr("%1 h").arg(Math.floor(s / 3600))
        if (s < 14 * 86400)
            return qsTr("%1 d").arg(Math.floor(s / 86400))
        return qsTr("%1 w").arg(Math.floor(s / (7 * 86400)))
    }

    height: 58

    Rectangle {
        anchors.fill: parent
        radius: Theme.rowRadius
        color: rowHover.hovered ? Theme.selection : "transparent"
    }
    Icon {
        id: glyph
        x: 12
        y: 12
        width: 16
        height: 16
        path: AgentGitGlyphs.pullRequest
        color: row.pr.draft === true ? Theme.surfaceContentVariant : AgentPalette.done
    }
    Column {
        anchors.left: glyph.right
        anchors.leftMargin: 10
        anchors.right: actions.left
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        spacing: 3
        Txt {
            width: parent.width
            text: row.pr.title || ""
            maximumLineCount: 1
        }
        Row {
            width: parent.width
            spacing: 8
            Txt {
                text: [row.pr.repo || "", "#" + (row.pr.number || 0) + (row.pr.draft === true ? " · " + qsTr("Draft") : "")]
                    .filter((s) => s.length > 0).join(" ")
                role: "bodySmall"
                muted: true
            }
            Row {
                visible: row.total > 0
                spacing: 4
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 12
                    height: 12
                    path: (row.checks.fail || 0) > 0 ? AgentGitGlyphs.failed
                        : (row.checks.pending || 0) > 0 ? AgentGitGlyphs.running : AgentGitGlyphs.passed
                    color: (row.checks.fail || 0) > 0 ? AgentPalette.failed
                        : (row.checks.pending || 0) > 0 ? AgentPalette.working : AgentPalette.done
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: (row.checks.fail || 0) > 0 ? qsTr("%1 of %2 failed").arg(row.checks.fail).arg(row.total)
                        : (row.checks.pending || 0) > 0 ? qsTr("%1 of %2 running").arg(row.checks.pending).arg(row.total)
                        : qsTr("%n checks passed", "", row.total)
                    role: "bodySmall"
                    muted: true
                }
            }
        }
    }
    Row {
        id: actions
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 4
        TextAction {
            visible: (row.pr.thread || "").length > 0
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Open thread")
            onClicked: row.openThread(row.pr.thread)
        }
        Txt {
            anchors.verticalCenter: parent.verticalCenter
            text: row.ago(row.pr.updatedMs || 0)
            role: "caption"
            muted: true
        }
        IconButton {
            id: copyButton
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.copy
            iconSize: 14
            label: qsTr("Copy link")
            doneLabel: qsTr("Copied")
            onClicked: {
                Agents.copy(row.pr.url || "")
                copyButton.flashDone()
            }
        }
        IconButton {
            anchors.verticalCenter: parent.verticalCenter
            enabled: row.pr.canOpen === true
            iconPath: Icons.openExternal
            iconSize: 14
            label: qsTr("Open on GitHub")
            onClicked: Agents.openPrUrl(row.pr.url || "")
        }
    }
    HoverHandler {
        id: rowHover
    }
}
