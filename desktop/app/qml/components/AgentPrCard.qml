// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The branch's pull request (§SC9.1): "#212 · Open" (Draft, Merged,
// Closed) and its title, the branch it goes into, the checks line (it opens
// the list, failing first), the review, "Open on GitHub", when it was read
// with Refresh; Merge… and Mark ready in G7 Link only. Never a check's log,
// the pull request's diff or a comment box.
Rectangle {
    id: card
    // Agents.git's pr.pr: { number, title, state, draft, url, base, oid7, review, conflicts,
    // checksLine, checksN, checksTotal, failed, running, checks: [{ name, state, url }], readMs }
    property var pr: ({})
    // GitHub couldn't be reached: what was read last, and when.
    property bool stale: false
    property bool refreshing: false
    property bool busy: false
    // "Changes and comments" opens the pull request's view (phase 6).
    property bool canDetail: false
    // In that view: nothing that writes to GitHub (no Merge, no Mark ready).
    property bool readOnly: false
    signal refresh
    signal openUrl(string url)
    signal merge
    signal ready
    signal details

    property bool expanded: false
    readonly property bool open: pr.state === "open"
    readonly property string stateWord: pr.draft === true && open ? qsTr("Draft")
        : pr.state === "merged" ? qsTr("Merged") : pr.state === "closed" ? qsTr("Closed") : qsTr("Open")
    readonly property string checksText: {
        switch (pr.checksLine) {
        case "passed": return qsTr("%n checks passed", "", pr.checksN || 0)
        case "failed": return qsTr("%1 of %2 failed").arg(pr.checksN || 0).arg(pr.checksTotal || 0)
        case "running": return qsTr("%1 of %2 running").arg(pr.checksN || 0).arg(pr.checksTotal || 0)
        default: return qsTr("No checks")
        }
    }
    readonly property string reviewText: {
        switch (pr.review) {
        case "approved": return qsTr("Approved")
        case "changes_requested": return qsTr("Changes requested")
        case "review_required": return qsTr("Review required")
        default: return ""
        }
    }
    function clock(ms) {
        return ms > 0 ? new Date(ms).toLocaleTimeString(Qt.locale(), Locale.ShortFormat) : ""
    }
    function checkPath(state) {
        switch (state) {
        case "pass": return AgentGitGlyphs.passed
        case "fail": return AgentGitGlyphs.failed
        case "pending": return AgentGitGlyphs.running
        default: return AgentGitGlyphs.waiting
        }
    }
    function checkTone(state) {
        switch (state) {
        case "pass": return AgentPalette.done
        case "fail": return AgentPalette.failed
        case "pending": return AgentPalette.working
        default: return AgentPalette.paused
        }
    }

    height: column.height + 20
    radius: Theme.radiusMd
    color: "transparent"
    border.width: 1
    border.color: Theme.outlineVariant

    Column {
        id: column
        x: 12
        y: 10
        width: parent.width - 24
        spacing: 8

        Item {
            width: parent.width
            height: 24
            Row {
                id: head
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                    path: card.pr.state === "merged" ? AgentGitGlyphs.merged : AgentGitGlyphs.pullRequest
                    color: card.pr.state === "merged" ? AgentPalette.done
                        : card.pr.state === "closed" ? AgentPalette.failed : Theme.surfaceContentVariant
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("#%1 · %2").arg(card.pr.number || 0).arg(card.stateWord)
                    role: "bodySmall"
                    weight: 600
                }
            }
            Txt {
                anchors.right: refreshButton.left
                anchors.rightMargin: 4
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Read %1").arg(card.clock(card.pr.readMs || 0))
                role: "caption"
                muted: true
            }
            IconButton {
                id: refreshButton
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                iconPath: Icons.refresh
                iconSize: 14
                label: qsTr("Refresh")
                enabled: !card.refreshing
                onClicked: card.refresh()
            }
        }
        Txt {
            width: parent.width
            text: card.pr.title || ""
            role: "bodySmall"
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
            maximumLineCount: 3
        }
        Txt {
            width: parent.width
            text: qsTr("into %1").arg(card.pr.base || "")
            role: "caption"
            muted: true
        }
        // The checks line: it opens the list.
        Item {
            visible: card.open
            width: parent.width
            height: 22
            activeFocusOnTab: (card.pr.checks || []).length > 0
            Accessible.role: Accessible.Button
            Accessible.name: card.checksText
            Accessible.onPressAction: card.expanded = !card.expanded
            Keys.onSpacePressed: card.expanded = !card.expanded
            Row {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 6
                Icon {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                    path: card.pr.checksLine === "failed" ? AgentGitGlyphs.failed
                        : card.pr.checksLine === "running" ? AgentGitGlyphs.running
                        : card.pr.checksLine === "passed" ? AgentGitGlyphs.passed : AgentGitGlyphs.waiting
                    color: card.pr.checksLine === "failed" ? AgentPalette.failed
                        : card.pr.checksLine === "running" ? AgentPalette.working
                        : card.pr.checksLine === "passed" ? AgentPalette.done : AgentPalette.paused
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: card.checksText
                    role: "bodySmall"
                }
                Icon {
                    visible: (card.pr.checks || []).length > 0
                    anchors.verticalCenter: parent.verticalCenter
                    width: 12
                    height: 12
                    path: card.expanded ? Icons.chevronUp : Icons.chevronDown
                    color: Theme.surfaceContentVariant
                }
            }
            TapHandler {
                enabled: (card.pr.checks || []).length > 0
                onTapped: card.expanded = !card.expanded
            }
        }
        Column {
            visible: card.expanded && card.open
            width: parent.width
            spacing: 2
            Repeater {
                model: card.expanded ? (card.pr.checks || []) : []
                delegate: Item {
                    id: check
                    required property var modelData
                    width: column.width
                    height: 22
                    Icon {
                        id: checkIcon
                        x: 4
                        anchors.verticalCenter: parent.verticalCenter
                        width: 13
                        height: 13
                        path: card.checkPath(check.modelData.state)
                        color: card.checkTone(check.modelData.state)
                    }
                    Txt {
                        anchors.left: checkIcon.right
                        anchors.leftMargin: 8
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        text: check.modelData.name || ""
                        role: "caption"
                        maximumLineCount: 1
                    }
                }
            }
        }
        Txt {
            visible: card.reviewText.length > 0 && card.open
            text: card.reviewText
            role: "caption"
            color: card.pr.review === "approved" ? AgentPalette.done
                : card.pr.review === "changes_requested" ? AgentPalette.failedText : Theme.surfaceContentVariant
        }
        Txt {
            visible: card.pr.conflicts === true && card.open
            width: parent.width
            text: qsTr("Has conflicts with %1").arg(card.pr.base || "")
            role: "caption"
            color: AgentPalette.failedText
            wrapMode: Text.WordWrap
        }
        Txt {
            visible: card.stale
            width: parent.width
            text: qsTr("GitHub couldn't be reached. Showing what was read at %1.").arg(card.clock(card.pr.readMs || 0))
            role: "caption"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Flow {
            width: parent.width
            spacing: 6
            Button {
                visible: card.canDetail
                size: "sm"
                variant: "tonal"
                iconPath: AgentGitGlyphs.pullRequest
                text: qsTr("Changes and comments")
                onClicked: card.details()
            }
            Button {
                visible: !card.readOnly || card.pr.canOpen !== false
                size: "sm"
                variant: "text"
                iconPath: Icons.openExternal
                text: qsTr("Open on GitHub")
                onClicked: card.openUrl(card.pr.url || "")
            }
            Button {
                visible: !card.readOnly && card.open && card.pr.draft === true
                size: "sm"
                variant: "tonal"
                text: qsTr("Mark ready")
                enabled: !card.busy
                onClicked: card.ready()
            }
            Button {
                visible: !card.readOnly && card.open && card.pr.draft !== true && !card.stale
                size: "sm"
                variant: "tonal"
                iconPath: AgentGitGlyphs.merged
                text: qsTr("Merge…")
                enabled: !card.busy
                onClicked: card.merge()
            }
        }
    }
}
