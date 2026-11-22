// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// A pull request, read-only (§X6.6): the card (state, title, base, checks,
// review, "Open on GitHub", when it was read), "Ask <agent> to review it",
// who opened it and the description, its changed files (each opens the
// diff), its checks, and its comments, oldest first. Comments made on lines
// of code aren't listed; the view says so. Nothing here posts to GitHub.
Item {
    id: view
    // Agents.prDetail, parsed.
    property var detail: ({})
    property real now: Date.now()
    signal linkActivated(string key, int index)
    // "Ask in a new thread…": the page shows the new-thread form with the chip.
    signal askInNewThread(string repo, int number, string folderId)

    readonly property var pr: detail.pr || ({})
    readonly property bool hasPr: (pr.number || 0) > 0
    readonly property var files: detail.files || []
    readonly property var comments: detail.comments || []
    readonly property var ask: detail.ask || ({})
    readonly property var checks: pr.checks || []
    readonly property string agentName: ask.agentName || ""

    function clock(ms) {
        if (ms <= 0)
            return ""
        const when = new Date(ms)
        return new Date(view.now).toDateString() === when.toDateString()
            ? when.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
            : when.toLocaleDateString(Qt.locale(), Locale.ShortFormat) + " " + when.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
    }
    function reviewWord(word) {
        switch (word) {
        case "approved": return qsTr("approved")
        case "changes_requested": return qsTr("requested changes")
        case "commented": return qsTr("reviewed")
        case "dismissed": return qsTr("review dismissed")
        default: return ""
        }
    }
    readonly property string whyText: {
        if (view.hasPr && detail.why !== "too_large")
            return ""
        switch ((detail.gh || {}).gh) {
        case "missing": return qsTr("Pull requests need the GitHub CLI.")
        case "signed_out": return qsTr("You're signed out of the GitHub CLI. Run:")
        case "old": return qsTr("Your GitHub CLI is too old for this. Update it.")
        default: break
        }
        switch (detail.why || "") {
        case "too_large": return qsTr("This pull request is too large to show here.")
        case "not_github": return qsTr("This folder's remote isn't on GitHub.")
        case "network": return qsTr("GitHub couldn't be reached. Try again.")
        default: return detail.failed === true ? qsTr("Something went wrong. Try again.") : ""
        }
    }

    // A section's name.
    component Heading: Txt {
        topPadding: 6
        role: "label"
        muted: true
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
    }
    Flickable {
        id: scroll
        anchors.fill: parent
        contentHeight: column.height + 32
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: column
            x: 16
            y: 16
            width: Math.min(760, scroll.width - 32)
            spacing: 12

            Row {
                visible: view.detail.loading === true
                spacing: 8
                Spinner {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                }
                Txt {
                    text: qsTr("Asking GitHub…")
                    role: "bodySmall"
                    muted: true
                }
            }
            Txt {
                visible: view.whyText.length > 0 && view.detail.loading !== true
                width: parent.width
                text: view.whyText
                role: "bodySmall"
                wrapMode: Text.WordWrap
                elide: Text.ElideNone
            }

            AgentPrCard {
                visible: view.hasPr
                width: parent.width
                pr: view.pr
                readOnly: true
                stale: view.detail.why === "network"
                refreshing: view.detail.refreshing === true
                onRefresh: Agents.prDetailRefresh()
                onOpenUrl: (url) => Agents.openPrUrl(url)
            }

            // ---- Ask <agent> to review it ----
            Column {
                visible: view.hasPr && view.ask.mode === "thread"
                width: parent.width
                spacing: 4
                Button {
                    variant: "tonal"
                    iconPath: AgentGitGlyphs.pullRequest
                    text: qsTr("Ask %1 to review it").arg(view.agentName)
                    enabled: view.ask.busy !== true
                    onClicked: Agents.prAskReview()
                }
                Txt {
                    width: parent.width
                    text: view.ask.busy === true ? qsTr("Wait until %1 finishes.").arg(view.agentName)
                        : qsTr("Sends one message in this thread and uses a little of your %1 plan. Nothing is posted to GitHub.").arg(view.agentName)
                    role: "caption"
                    muted: true
                    wrapMode: Text.WordWrap
                    elide: Text.ElideNone
                }
            }
            Button {
                visible: view.hasPr && view.ask.mode === "new"
                variant: "tonal"
                iconPath: Icons.plus
                text: qsTr("Ask in a new thread…")
                onClicked: view.askInNewThread(view.detail.repo || "", view.pr.number || 0, view.ask.folderId || "")
            }

            // ---- Who opened it, and the description ----
            Column {
                visible: view.hasPr
                width: parent.width
                spacing: 8
                Txt {
                    visible: (view.detail.author || "").length > 0
                    width: parent.width
                    text: qsTr("opened by %1").arg(view.detail.author || "")
                    role: "bodySmall"
                    muted: true
                    maximumLineCount: 1
                }
                AgentPrComment {
                    width: parent.width
                    blocks: view.detail.body || []
                    onLinkActivated: (key, index) => view.linkActivated(key, index)
                }
                Txt {
                    visible: (view.detail.body || []).length === 0
                    text: qsTr("No description.")
                    role: "bodySmall"
                    muted: true
                }
                Txt {
                    visible: view.detail.bodyCut === true
                    text: qsTr("Shortened to fit.")
                    role: "caption"
                    muted: true
                }
            }

            // ---- Changes ----
            Column {
                visible: view.hasPr
                width: parent.width
                spacing: 1
                Item {
                    width: parent.width
                    height: 28
                    Heading {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Changes") + " · " + qsTr("%n files changed", "", view.detail.changed || 0)
                    }
                    Row {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Txt {
                            text: "+" + (view.detail.adds || 0)
                            role: "code"
                            size: Theme.typography.caption.size
                            color: AgentPalette.inserted
                        }
                        Txt {
                            text: "−" + (view.detail.dels || 0)
                            role: "code"
                            size: Theme.typography.caption.size
                            color: AgentPalette.deleted
                        }
                    }
                }
                Repeater {
                    model: view.files
                    delegate: Rectangle {
                        id: fileRow
                        required property var modelData
                        width: column.width
                        height: 30
                        radius: Theme.radiusSm
                        color: fileHover.hovered || fileRow.activeFocus ? Theme.selection : "transparent"
                        activeFocusOnTab: true
                        Accessible.role: Accessible.Button
                        Accessible.name: fileRow.modelData.path
                        Accessible.onPressAction: Agents.prDetailFile(fileRow.modelData.path)
                        Keys.onReturnPressed: Agents.prDetailFile(fileRow.modelData.path)
                        Keys.onSpacePressed: Agents.prDetailFile(fileRow.modelData.path)
                        HoverHandler {
                            id: fileHover
                            cursorShape: Qt.PointingHandCursor
                        }
                        TapHandler {
                            onTapped: Agents.prDetailFile(fileRow.modelData.path)
                        }
                        Txt {
                            id: letter
                            x: 8
                            width: 14
                            anchors.verticalCenter: parent.verticalCenter
                            text: fileRow.modelData.letter || "M"
                            role: "code"
                            size: Theme.typography.caption.size
                            color: fileRow.modelData.change === "added" ? AgentPalette.inserted
                                : fileRow.modelData.change === "deleted" ? AgentPalette.deleted : Theme.surfaceContentVariant
                        }
                        Txt {
                            anchors.left: letter.right
                            anchors.leftMargin: 8
                            anchors.right: counts.left
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            text: fileRow.modelData.path
                            role: "code"
                            size: Theme.typography.bodySmall.size
                            maximumLineCount: 1
                            elide: Text.ElideMiddle
                        }
                        Row {
                            id: counts
                            anchors.right: parent.right
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 6
                            Txt {
                                text: "+" + (fileRow.modelData.adds || 0)
                                role: "code"
                                size: Theme.typography.caption.size
                                color: AgentPalette.inserted
                            }
                            Txt {
                                text: "−" + (fileRow.modelData.dels || 0)
                                role: "code"
                                size: Theme.typography.caption.size
                                color: AgentPalette.deleted
                            }
                        }
                    }
                }
                Txt {
                    visible: (view.detail.filesMore || 0) > 0
                    x: 8
                    height: 24
                    verticalAlignment: Text.AlignVCenter
                    text: qsTr("%1 more files aren't listed.").arg(view.detail.filesMore || 0)
                    role: "bodySmall"
                    muted: true
                }
            }

            // ---- Checks ----
            Column {
                visible: view.hasPr && view.checks.length > 0
                width: parent.width
                spacing: 2
                Heading {
                    text: qsTr("Checks")
                }
                Repeater {
                    model: view.checks
                    delegate: Item {
                        id: check
                        required property var modelData
                        width: column.width
                        height: 24
                        Icon {
                            id: checkIcon
                            x: 8
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13
                            height: 13
                            path: check.modelData.state === "pass" ? AgentGitGlyphs.passed
                                : check.modelData.state === "fail" ? AgentGitGlyphs.failed
                                : check.modelData.state === "pending" ? AgentGitGlyphs.running : AgentGitGlyphs.waiting
                            color: check.modelData.state === "pass" ? AgentPalette.done
                                : check.modelData.state === "fail" ? AgentPalette.failed
                                : check.modelData.state === "pending" ? AgentPalette.working : AgentPalette.paused
                        }
                        Txt {
                            anchors.left: checkIcon.right
                            anchors.leftMargin: 8
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: check.modelData.name || ""
                            role: "bodySmall"
                            maximumLineCount: 1
                        }
                    }
                }
            }

            // ---- Comments ----
            Column {
                visible: view.hasPr
                width: parent.width
                spacing: 10
                Heading {
                    text: qsTr("Comments")
                }
                Txt {
                    visible: (view.detail.commentsOlder || 0) > 0
                    text: qsTr("%n older comments aren't shown.", "", view.detail.commentsOlder || 0)
                    role: "bodySmall"
                    muted: true
                }
                Txt {
                    visible: view.comments.length === 0
                    text: qsTr("No comments yet.")
                    role: "bodySmall"
                    muted: true
                }
                Repeater {
                    model: view.comments
                    delegate: Column {
                        id: one
                        required property var modelData
                        width: column.width
                        spacing: 4
                        Txt {
                            width: parent.width
                            text: [one.modelData.author || "", view.reviewWord(one.modelData.review || ""),
                                   view.clock(one.modelData.atMs || 0)].filter((x) => x.length > 0).join(" · ")
                            role: "bodySmall"
                            weight: 500
                            maximumLineCount: 1
                        }
                        AgentPrComment {
                            width: parent.width
                            blocks: one.modelData.blocks || []
                            onLinkActivated: (key, index) => view.linkActivated(key, index)
                        }
                        Txt {
                            visible: one.modelData.cut === true
                            text: qsTr("Shortened to fit.")
                            role: "caption"
                            muted: true
                        }
                    }
                }
                Row {
                    width: parent.width
                    spacing: 8
                    Icon {
                        width: 14
                        height: 14
                        path: Icons.info
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        width: parent.width - 22
                        text: qsTr("Comments made on lines of code aren't shown here. Read them on GitHub.")
                        role: "bodySmall"
                        muted: true
                        wrapMode: Text.WordWrap
                        elide: Text.ElideNone
                    }
                }
            }
        }
        TouchpadGlide { view: scroll }
    }
    G7ScrollBar {
        flickable: scroll
        anchors.top: scroll.top
        anchors.bottom: scroll.bottom
        anchors.right: scroll.right
    }
}
