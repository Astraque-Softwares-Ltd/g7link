// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The thread's source control (§SC9.1), beside the transcript: the branch
// with Refresh and when it was checked; where it stands against its remote;
// a state line when one applies; Changes (tick, status letter, path, +/−)
// and the commit box; Push; the pull request, or why there is none; and
// the running or last job's output. Git and the GitHub CLI run on the host;
// this only shows what it read and asks it to act on a press.
Item {
    id: panel
    // Agents.git, parsed.
    property var git: ({})
    property real now: Date.now()
    signal closed
    signal branchMenuAt(real x, real y)
    signal switchAsked(string branch, int files)
    signal pushMenuAt(real x, real y, var actions)
    signal removeWorktree(string branch, int unpushed, int dirty)
    signal openFile(string path)

    readonly property var st: git.status || ({})
    readonly property bool repo: st.repo === "ok"
    readonly property var files: st.files || []
    readonly property var job: git.job || ({})
    readonly property bool jobRunning: job.running === true
    readonly property var commit: git.commit || ({})
    readonly property var prState: git.pr || ({})
    readonly property var pr: prState.pr || ({})
    readonly property bool hasPr: (pr.number || 0) > 0
    readonly property var gh: prState.gh || ({})
    readonly property var refused: git.refused || ({})
    readonly property string agentName: git.agentName || ""
    readonly property bool worktree: (st.worktree || {}).id !== undefined
    readonly property int ahead: st.ahead ?? -1
    readonly property int behind: st.behind ?? -1
    readonly property bool hasUpstream: (st.upstream || "").length > 0
    readonly property bool canPush: repo && !st.unborn && (st.detached || "").length === 0
        && (ahead > 0 || !hasUpstream) && (st.remotes || []).length > 0
    readonly property int ticked: files.filter((f) => f.ticked === true).length
    readonly property int tickable: files.filter((f) => f.conflict !== true).length

    function clock(ms) {
        return ms > 0 ? new Date(ms).toLocaleTimeString(Qt.locale(), Locale.ShortFormat) : ""
    }
    // The state line: what stops git here, in words ("" for nothing).
    readonly property string stateLine: {
        if (git.loading === true)
            return ""
        switch (st.repo) {
        case "none": return qsTr("This folder isn't a git repository.") + " " + qsTr("Source control and restore points need one.")
        case "no_git": return qsTr("Git isn't installed on this PC.")
        case "refused": return qsTr("Git won't work in this folder. It says the folder isn't safe.")
        default: break
        }
        if (!repo)
            return git.failed === true ? qsTr("Something went wrong. Try again.") : ""
        if (st.locked === true)
            return qsTr("Git is busy in this folder. Try again in a moment.")
        if ((st.doing || "").length > 0)
            return qsTr("A %1 is in progress here. Finish it in a terminal.").arg(st.doing.replace("_", "-"))
        if ((st.detached || "").length > 0)
            return qsTr("You're not on a branch. Create one to keep new commits.")
        if ((st.conflicts || 0) > 0)
            return qsTr("%n files have conflicts. Resolve them first.", "", st.conflicts)
        return ""
    }
    // A request the host refused before anything ran, in words.
    readonly property string refusedLine: {
        switch (refused.reason || "") {
        case "": return ""
        case "changed": return qsTr("The files changed while you were looking.")
        case "identity": return qsTr("Git doesn't know your name and email. Set them in a terminal:")
        case "no_remote": return qsTr("This repository has no remote.")
        case "pick_remote": return ""
        case "no_commits": return qsTr("No commits yet.")
        case "detached": return qsTr("You're not on a branch. Create one to keep new commits.")
        case "git_busy": return qsTr("Git is busy in this folder. Try again in a moment.")
        case "doing": return qsTr("A merge or rebase is in progress here. Finish it in a terminal.")
        case "conflicts": return qsTr("%n files have conflicts. Resolve them first.", "", st.conflicts || 1)
        case "nothing": return qsTr("Nothing to commit.")
        case "thread_busy": return qsTr("Wait until %1 finishes.").arg(panel.agentName)
        case "shared_busy": return qsTr("Another thread is working in this folder.")
        case "jobs": return qsTr("Wait until it finishes.")
        case "in_worktree": return qsTr("This thread works in its own worktree.")
        case "exists": return qsTr("A branch with that name exists.")
        case "bad_name": return qsTr("That name can't be a branch.")
        case "dirty": return qsTr("This folder has changes.")
        case "not_pushed": return qsTr("Push this branch to open a pull request.")
        case "not_github": return qsTr("This folder's remote isn't on GitHub.")
        case "no_gh": return qsTr("Pull requests need the GitHub CLI.")
        case "signed_out": return qsTr("You're signed out of the GitHub CLI. Run:")
        case "old_gh": return qsTr("Your GitHub CLI is too old for this. Update it.")
        case "pr_exists": return qsTr("This branch already has a pull request.")
        case "denied": return qsTr("That is done from G7 Link on this PC.")
        default: return qsTr("Something went wrong. Try again.")
        }
    }
    // Why there is no pull request card.
    readonly property string prWhy: {
        if (panel.hasPr || prState.loading === true)
            return ""
        switch (panel.gh.gh) {
        case "missing": return qsTr("Pull requests need the GitHub CLI.")
        case "signed_out": return qsTr("You're signed out of the GitHub CLI. Run:")
        case "old": return qsTr("Your GitHub CLI is too old for this. Update it.")
        default: break
        }
        switch (prState.why) {
        case "not_github": return qsTr("This folder's remote isn't on GitHub.")
        case "no_upstream": return qsTr("Push this branch to open a pull request.")
        case "detached": return qsTr("You're not on a branch. Create one to keep new commits.")
        case "network": return qsTr("GitHub couldn't be reached. Try again.")
        case "not_repo": return ""
        default: return ""
        }
    }

    // A command to run in a terminal, with Copy.
    component CommandLine: Rectangle {
        id: commandLine
        property string command
        height: 30
        radius: Theme.radiusSm
        color: AgentPalette.codeGround
        Txt {
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 48
            text: commandLine.command
            role: "code"
            size: Theme.typography.caption.size
            maximumLineCount: 1
        }
        IconButton {
            anchors.right: parent.right
            anchors.rightMargin: 2
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.copy
            iconSize: 13
            label: qsTr("Copy")
            onClicked: Agents.copy(commandLine.command)
        }
    }
    // A line with an icon and words.
    component Note: Row {
        id: note
        property string text
        property string iconPath: Icons.info
        // Something stops git here: drawn in the warning colour.
        property bool warn: false
        spacing: 8
        Icon {
            width: 14
            height: 14
            path: note.iconPath
            color: note.warn ? Theme.warning : Theme.surfaceContentVariant
        }
        Txt {
            width: note.width - 22
            text: note.text
            role: "bodySmall"
            color: note.warn ? Theme.warning : Theme.surfaceContent
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
    }
    Item {
        id: head
        width: parent.width
        height: 36
        Txt {
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Source control")
            role: "label"
            muted: true
        }
        IconButton {
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            iconPath: Icons.close
            iconSize: 14
            label: qsTr("Close")
            onClicked: panel.closed()
        }
    }
    Flickable {
        id: scroll
        anchors.top: head.bottom
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        contentHeight: column.height + 16
        boundsBehavior: Flickable.StopAtBounds
        clip: true

        Column {
            id: column
            x: 14
            width: scroll.width - 28
            spacing: 12

            // ---- The branch, when it was checked, Refresh ----
            Item {
                visible: panel.repo
                width: parent.width
                height: 30
                Rectangle {
                    id: branchButton
                    width: Math.min(branchRow.implicitWidth + 20, parent.width - checked.width - 40)
                    height: 30
                    radius: Theme.radiusSm
                    color: branchHover.hovered ? Theme.selection : Theme.raised
                    border.width: branchButton.activeFocus ? 1 : 0
                    border.color: Theme.focusRing
                    activeFocusOnTab: true
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Switch branch")
                    Accessible.onPressAction: branchButton.openMenu()
                    Keys.onReturnPressed: branchButton.openMenu()
                    Keys.onSpacePressed: branchButton.openMenu()
                    function openMenu() {
                        const p = branchButton.mapToItem(null, 0, branchButton.height + 4)
                        panel.branchMenuAt(p.x, p.y)
                    }
                    Row {
                        id: branchRow
                        x: 10
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 8
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 14
                            height: 14
                            path: panel.worktree ? Icons.folder : AgentGitGlyphs.branch
                            color: Theme.surfaceContentVariant
                        }
                        Txt {
                            anchors.verticalCenter: parent.verticalCenter
                            width: Math.min(implicitWidth, branchButton.width - 58)
                            text: (panel.st.branch || "").length > 0 ? panel.st.branch
                                : qsTr("Detached at %1").arg(panel.st.detached || "")
                            role: "code"
                            size: Theme.typography.bodySmall.size
                            maximumLineCount: 1
                        }
                        Icon {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 13
                            height: 13
                            path: Icons.chevronDown
                            color: Theme.surfaceContentVariant
                        }
                    }
                    HoverHandler {
                        id: branchHover
                        cursorShape: Qt.PointingHandCursor
                    }
                    TapHandler {
                        onTapped: branchButton.openMenu()
                    }
                }
                Spinner {
                    visible: panel.git.fetching === true
                    anchors.right: checked.left
                    anchors.rightMargin: 6
                    anchors.verticalCenter: parent.verticalCenter
                    width: 12
                    height: 12
                }
                Txt {
                    id: checked
                    anchors.right: refreshButton.left
                    anchors.rightMargin: 2
                    anchors.verticalCenter: parent.verticalCenter
                    visible: (panel.st.checkedMs || 0) > 0
                    text: qsTr("Checked %1").arg(panel.clock(panel.st.checkedMs || 0))
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
                    enabled: panel.git.fetching !== true
                    onClicked: Agents.gitRefresh()
                }
            }
            // Where the branch stands against its remote.
            Flow {
                visible: panel.repo && !panel.st.unborn
                width: parent.width
                spacing: 12
                Row {
                    visible: panel.ahead > 0
                    spacing: 3
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 12
                        height: 12
                        path: AgentGitGlyphs.up
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        text: qsTr("%n to push", "", Math.max(0, panel.ahead))
                        role: "bodySmall"
                        muted: true
                    }
                }
                Row {
                    visible: panel.behind > 0
                    spacing: 3
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 12
                        height: 12
                        path: AgentGitGlyphs.down
                        color: Theme.surfaceContentVariant
                    }
                    Txt {
                        text: qsTr("%n behind, as of the last fetch", "", Math.max(0, panel.behind))
                        role: "bodySmall"
                        muted: true
                    }
                }
                Txt {
                    visible: panel.hasUpstream && panel.ahead <= 0 && panel.behind <= 0
                    text: qsTr("In step with %1").arg(panel.st.remote || "")
                    role: "bodySmall"
                    muted: true
                }
                Txt {
                    visible: !panel.hasUpstream && (panel.st.detached || "").length === 0
                    text: qsTr("Not on a remote yet")
                    role: "bodySmall"
                    muted: true
                }
            }
            // A worktree thread: its branch is its worktree.
            Item {
                visible: panel.worktree
                width: parent.width
                height: Math.max(worktreeNote.implicitHeight, removeAction.height)
                Note {
                    id: worktreeNote
                    anchors.left: parent.left
                    anchors.right: removeAction.left
                    anchors.rightMargin: 8
                    anchors.verticalCenter: parent.verticalCenter
                    iconPath: Icons.folder
                    text: qsTr("This thread works in its own worktree.")
                }
                TextAction {
                    id: removeAction
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Remove")
                    enabled: panel.git.running !== true
                    onClicked: panel.removeWorktree((panel.st.worktree || {}).branch || panel.st.branch || "",
                                                    Math.max(0, panel.ahead), panel.files.length)
                }
            }

            // ---- Loading, or the state line ----
            Row {
                visible: panel.git.loading === true
                spacing: 8
                Spinner {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 14
                    height: 14
                }
                Txt {
                    text: qsTr("Reading the folder…")
                    role: "bodySmall"
                    muted: true
                }
            }
            Note {
                visible: panel.stateLine.length > 0
                width: parent.width
                iconPath: Icons.warning
                warn: panel.st.repo === "ok"
                text: panel.stateLine
            }
            TextAction {
                visible: panel.repo && (panel.st.detached || "").length > 0
                iconPath: Icons.plus
                text: qsTr("New branch…")
                onClicked: Agents.gitNewBranchThenPush()
            }
            // A refusal, with what helps.
            Column {
                visible: panel.refusedLine.length > 0
                width: parent.width
                spacing: 6
                Note {
                    width: parent.width
                    iconPath: Icons.warning
                    warn: true
                    text: panel.refusedLine
                }
                CommandLine {
                    visible: panel.refused.reason === "identity"
                    width: parent.width
                    command: "git config --global user.name \"…\""
                }
                CommandLine {
                    visible: panel.refused.reason === "identity"
                    width: parent.width
                    command: "git config --global user.email \"…\""
                }
                TextAction {
                    visible: panel.refused.reason === "changed"
                    iconPath: Icons.refresh
                    text: qsTr("Reload")
                    onClicked: Agents.gitReload()
                }
            }

            // ---- Changes ----
            Column {
                visible: panel.repo && (panel.st.doing || "").length === 0
                width: parent.width
                spacing: 1
                Item {
                    width: parent.width
                    height: 26
                    Rectangle {
                        id: allBox
                        visible: panel.files.length > 0
                        x: 8
                        anchors.verticalCenter: parent.verticalCenter
                        width: 14
                        height: 14
                        radius: 3
                        readonly property bool all: panel.ticked > 0 && panel.ticked === panel.tickable
                        color: allBox.all ? Theme.accent : "transparent"
                        border.width: allBox.all ? 0 : 1
                        border.color: allBox.activeFocus ? Theme.focusRing : Theme.outline
                        activeFocusOnTab: visible && !panel.jobRunning
                        Accessible.role: Accessible.CheckBox
                        Accessible.name: qsTr("Select all")
                        Accessible.checkable: true
                        Accessible.checked: allBox.all
                        Accessible.onToggleAction: Agents.gitTickAll(!allBox.all)
                        Keys.onSpacePressed: Agents.gitTickAll(!allBox.all)
                        Icon {
                            visible: allBox.all || panel.ticked > 0
                            anchors.centerIn: parent
                            width: 11
                            height: 11
                            path: allBox.all ? Icons.check : "M5 12h14"
                            color: allBox.all ? Theme.accentContent : Theme.surfaceContentVariant
                            stroke: 2.4
                        }
                        TapHandler {
                            enabled: !panel.jobRunning
                            onTapped: Agents.gitTickAll(!allBox.all)
                        }
                    }
                    Txt {
                        x: panel.files.length > 0 ? 30 : 0
                        anchors.verticalCenter: parent.verticalCenter
                        text: panel.files.length > 0 ? qsTr("Changes") + " · " + qsTr("%n files", "", panel.st.total || panel.files.length)
                                                     : qsTr("Changes")
                        role: "label"
                        muted: true
                    }
                    Row {
                        visible: panel.files.length > 0
                        anchors.right: parent.right
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 6
                        Txt {
                            text: "+" + (panel.st.adds || 0)
                            role: "code"
                            size: Theme.typography.caption.size
                            color: AgentPalette.inserted
                        }
                        Txt {
                            text: "−" + (panel.st.dels || 0)
                            role: "code"
                            size: Theme.typography.caption.size
                            color: AgentPalette.deleted
                        }
                    }
                }
                Repeater {
                    model: panel.files
                    delegate: AgentGitFileRow {
                        required property var modelData
                        width: column.width
                        file: modelData
                        editable: !panel.jobRunning
                        onTicked: (on) => Agents.gitTick(modelData.path, on)
                        onOpened: panel.openFile(modelData.path)
                    }
                }
                Txt {
                    visible: (panel.st.unlisted || 0) > 0
                    x: 8
                    height: 24
                    verticalAlignment: Text.AlignVCenter
                    text: qsTr("%1 more files aren't listed.").arg(panel.st.unlisted || 0)
                    role: "bodySmall"
                    muted: true
                }
                Column {
                    visible: panel.files.length === 0 && panel.git.loading !== true
                    width: parent.width
                    spacing: 2
                    Txt {
                        x: 8
                        text: panel.st.unborn === true ? qsTr("No commits yet.") : qsTr("Nothing to commit.")
                        role: "bodySmall"
                    }
                    Txt {
                        visible: (panel.st.last || {}).subject !== undefined
                        x: 8
                        width: parent.width - 16
                        text: qsTr("Last commit: %1").arg((panel.st.last || {}).subject || "")
                        role: "bodySmall"
                        muted: true
                        maximumLineCount: 2
                        wrapMode: Text.WordWrap
                    }
                }
            }
            AgentCommitBox {
                visible: panel.repo && panel.files.length > 0 && (panel.st.doing || "").length === 0
                width: parent.width
                commit: panel.commit
                agentName: panel.agentName
                picked: panel.git.picked || 0
                total: panel.st.total || 0
                canAsk: panel.git.running !== true && panel.git.readonly !== true
                onEdited: (text) => Agents.gitField("message", text)
                onUseTitle: Agents.gitUseTitle()
                onAsk: Agents.gitAsk("commit")
                onUseOffer: Agents.gitUseOffer("message")
                onCommitted: (all) => Agents.gitCommit(all)
            }
            AgentPushButton {
                visible: panel.canPush || (panel.git.pickRemote || []).length > 0
                width: parent.width
                ahead: Math.max(0, panel.ahead)
                branch: panel.st.branch || ""
                onDefault: panel.st.onDefault === true
                remotes: (panel.git.pickRemote || []).map((name) => ({
                    name: name,
                    label: ((panel.st.remotes || []).find((r) => r.name === name) || {}).label || ""
                }))
                busy: panel.jobRunning
                onPushed: (remote) => Agents.gitPush(remote)
                onNewBranchFirst: Agents.gitNewBranchThenPush()
                onMenuAt: (x, y, actions) => panel.pushMenuAt(x, y, actions)
            }

            // ---- The job's output ----
            AgentJobOutput {
                visible: panel.job.kind !== undefined && panel.job.kind !== "fetch"
                width: parent.width
                job: panel.job
                agentName: panel.agentName
                onStopped: Agents.gitStop()
            }

            // ---- The pull request ----
            Column {
                visible: panel.repo
                width: parent.width
                spacing: 8
                Item {
                    width: parent.width
                    height: 26
                    Txt {
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Pull request")
                        role: "label"
                        muted: true
                    }
                    TextAction {
                        visible: !panel.hasPr && panel.prWhy.length === 0 && panel.gh.gh === "ok"
                                 && (panel.st.branch || "").length > 0 && panel.prState.loading !== true
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        iconPath: AgentGitGlyphs.pullRequest
                        text: qsTr("Open pull request")
                        onClicked: Agents.prForm(true)
                    }
                }
                Row {
                    visible: panel.prState.loading === true
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
                AgentPrCard {
                    visible: panel.hasPr
                    width: parent.width
                    pr: panel.pr
                    stale: panel.prState.why === "network"
                    refreshing: panel.prState.refreshing === true
                    busy: panel.jobRunning
                    onRefresh: Agents.gitRefresh()
                    onOpenUrl: (url) => Agents.openPrUrl(url)
                    onMerge: Agents.prMergeOpen()
                    onReady: Agents.prReady()
                }
                Rectangle {
                    visible: panel.prWhy.length > 0
                    width: parent.width
                    height: whyColumn.height + 20
                    radius: Theme.radiusMd
                    color: "transparent"
                    border.width: 1
                    border.color: Theme.outlineVariant
                    Column {
                        id: whyColumn
                        x: 12
                        y: 10
                        width: parent.width - 24
                        spacing: 8
                        Note {
                            width: parent.width
                            text: panel.prWhy
                        }
                        Txt {
                            visible: panel.gh.gh === "missing"
                            text: qsTr("Install it, then run:")
                            role: "bodySmall"
                            muted: true
                        }
                        CommandLine {
                            visible: panel.gh.gh === "missing" || panel.gh.gh === "signed_out"
                            width: parent.width
                            command: "gh auth login"
                        }
                    }
                }
            }
        }
    }
    G7ScrollBar {
        flickable: scroll
        anchors.top: scroll.top
        anchors.bottom: scroll.bottom
        anchors.right: scroll.right
    }
}
