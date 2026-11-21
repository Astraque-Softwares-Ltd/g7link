// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// The new-thread form's "Work in" (§SC3.8, §SC9.2): this folder, or a new
// worktree on its own branch (`g7/` and the message's first words, which
// you can change), starting from a branch of the folder. It starts from
// what's committed; files git ignores aren't copied. While git makes it,
// its lines show; when it fails, why.
Column {
    id: fields
    property string folderId
    // Agents.gitSetup: { available, basesFolder, bases: [{ name, current }], creating: { running, lines, reason } }
    property var gitSetup: ({})
    // The message, for the branch's name.
    property string words
    signal basesMenu(real x, real y, var actions)

    readonly property bool available: gitSetup.available === true
    readonly property bool worktree: available && workIn.value === "worktree"
    property bool branchEdited: false
    readonly property string branch: branchEdited ? branchField.text.trim() : Agents.worktreeBranch(words)
    readonly property bool branchOk: Agents.branchNameOk(branch)
    property string base: ""
    readonly property var bases: gitSetup.basesFolder === folderId ? (gitSetup.bases || []) : []
    readonly property string baseShown: base.length > 0 ? base
        : ((bases.find((b) => b.current === true) || {}).name || qsTr("Current commit"))
    readonly property var creating: gitSetup.creating || ({})

    onFolderIdChanged: {
        base = ""
        if (worktree)
            Agents.readBases(folderId)
    }
    onWorktreeChanged: if (worktree) Agents.readBases(folderId)
    onBranchChanged: if (!branchEdited && branchField.text !== branch) branchField.text = branch

    visible: available
    spacing: 10

    Column {
        width: parent.width
        spacing: 6
        Txt {
            text: qsTr("Work in")
            role: "label"
            muted: true
        }
        Segmented {
            id: workIn
            options: [
                { value: "folder", label: qsTr("This folder") },
                { value: "worktree", label: qsTr("A new worktree") }
            ]
            value: "folder"
            onPicked: (value) => {
                workIn.value = value
                Agents.creatingClear()
            }
        }
    }
    Row {
        visible: fields.worktree
        width: parent.width
        spacing: 16
        Column {
            width: Math.min(280, parent.width / 2)
            spacing: 6
            Txt {
                text: qsTr("Branch")
                role: "label"
                muted: true
            }
            Field {
                id: branchField
                width: parent.width
                text: fields.branch
                onTextChanged: if (branchField.input.activeFocus) fields.branchEdited = true
            }
        }
        Column {
            spacing: 6
            Txt {
                text: qsTr("Starts from")
                role: "label"
                muted: true
            }
            Chip {
                id: baseChip
                interactive: fields.bases.length > 0
                iconPath: AgentGitGlyphs.branch
                text: fields.baseShown
                onClicked: {
                    const p = baseChip.mapToItem(null, 0, baseChip.height)
                    fields.basesMenu(p.x, p.y, fields.bases.map((b) => ({
                        label: b.name,
                        icon: AgentGitGlyphs.branch,
                        checked: (fields.base.length === 0 && b.current === true) || fields.base === b.name,
                        run: () => { fields.base = b.current === true ? "" : b.name }
                    })))
                }
            }
        }
    }
    Txt {
        visible: fields.worktree && fields.branch.length > 0 && !fields.branchOk
        width: parent.width
        text: qsTr("That name can't be a branch.")
        role: "bodySmall"
        color: AgentPalette.failedText
    }
    Row {
        visible: fields.worktree
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
            text: qsTr("Starts from what's committed. Files git ignores, like .env or node_modules, aren't copied.")
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
    }
    AgentJobOutput {
        visible: fields.creating.running === true || (fields.creating.reason || "").length > 0
        width: parent.width
        job: ({
            lines: fields.creating.lines || [],
            running: fields.creating.running === true,
            phase: fields.creating.running === true ? "running" : "failed",
            words: fields.creating.running === true ? "creating_worktree" : "worktree_failed",
            args: [],
            dropped: 0
        })
    }
}
