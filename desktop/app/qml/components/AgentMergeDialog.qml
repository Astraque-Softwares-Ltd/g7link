// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Merge a pull request (§SC9.2), in G7 Link only: the methods the
// repository allows (the last one used here is picked), the head commit it
// is pinned to, the checks. Failing or running checks warn and need the
// tick "I've looked". Cancel has the keyboard; Merge takes a click, or Tab
// then Enter, never a bare Enter.
Sheet {
    id: dialog
    // Agents.git's merge: { number, oid7, loading, failed, methods, method, looked }.
    property var merge: ({})
    // The card's pull request, for the base and the checks.
    property var pr: ({})

    readonly property bool shown: (merge.number || 0) > 0
    readonly property bool warned: (pr.failed || 0) > 0 || (pr.running || 0) > 0
    readonly property bool ready: (merge.method || "").length > 0 && (!warned || merge.looked === true)
    readonly property string checksText: {
        switch (pr.checksLine) {
        case "passed": return qsTr("%n checks passed", "", pr.checksN || 0)
        case "failed": return qsTr("%1 of %2 failed").arg(pr.checksN || 0).arg(pr.checksTotal || 0)
        case "running": return qsTr("%1 of %2 running").arg(pr.checksN || 0).arg(pr.checksTotal || 0)
        default: return qsTr("No checks")
        }
    }

    cardWidth: 480
    onShownChanged: {
        if (shown) {
            open()
            Qt.callLater(() => cancelButton.forceActiveFocus(Qt.OtherFocusReason))
        } else {
            close()
        }
    }
    onDismissed: Agents.prMergeClose()

    Column {
        width: parent.width
        spacing: 14
        Txt {
            width: parent.width
            text: qsTr("Merge #%1 into %2?").arg(dialog.merge.number || 0).arg(dialog.pr.base || "")
            role: "title"
            wrapMode: Text.WordWrap
        }
        Spinner {
            visible: dialog.merge.loading === true
            width: 18
            height: 18
        }
        Txt {
            visible: dialog.merge.failed === true
            width: parent.width
            text: qsTr("GitHub couldn't be reached. Try again.")
            role: "bodySmall"
            color: AgentPalette.failedText
            wrapMode: Text.WordWrap
        }
        Segmented {
            visible: (dialog.merge.methods || []).length > 0
            options: (dialog.merge.methods || []).map((m) => ({
                value: m,
                label: m === "squash" ? qsTr("Squash and merge") : m === "rebase" ? qsTr("Rebase and merge") : qsTr("Merge commit")
            }))
            value: dialog.merge.method || ""
            onPicked: (value) => Agents.prMergeMethod(value)
        }
        Txt {
            width: parent.width
            text: qsTr("Merges %1, the commit you see here. If the branch moves first, nothing is merged.").arg(dialog.merge.oid7 || "")
            role: "bodySmall"
            muted: true
            wrapMode: Text.WordWrap
            elide: Text.ElideNone
        }
        Txt {
            width: parent.width
            text: [dialog.checksText,
                   dialog.pr.review === "approved" ? qsTr("Approved")
                   : dialog.pr.review === "changes_requested" ? qsTr("Changes requested")
                   : dialog.pr.review === "review_required" ? qsTr("Review required") : ""]
                .filter((s) => s.length > 0).join(" · ")
            role: "bodySmall"
        }
        Column {
            visible: dialog.warned
            width: parent.width
            spacing: 6
            Txt {
                width: parent.width
                text: (dialog.pr.failed || 0) > 0
                    ? qsTr("%n checks failed. Merge anyway?", "", dialog.pr.failed || 0)
                    : qsTr("%n checks are still running.", "", dialog.pr.running || 0)
                role: "bodySmall"
                color: (dialog.pr.failed || 0) > 0 ? AgentPalette.failedText : Theme.surfaceContent
                wrapMode: Text.WordWrap
            }
            Row {
                spacing: 8
                Rectangle {
                    id: lookedBox
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16
                    height: 16
                    radius: 3
                    color: dialog.merge.looked === true ? Theme.accent : "transparent"
                    border.width: dialog.merge.looked === true ? 0 : 1
                    border.color: lookedBox.activeFocus ? Theme.focusRing : Theme.outline
                    activeFocusOnTab: true
                    Accessible.role: Accessible.CheckBox
                    Accessible.name: qsTr("I've looked")
                    Accessible.checkable: true
                    Accessible.checked: dialog.merge.looked === true
                    Accessible.onToggleAction: Agents.prMergeLooked(dialog.merge.looked !== true)
                    Keys.onSpacePressed: Agents.prMergeLooked(dialog.merge.looked !== true)
                    Icon {
                        visible: dialog.merge.looked === true
                        anchors.centerIn: parent
                        width: 12
                        height: 12
                        path: Icons.check
                        color: Theme.accentContent
                        stroke: 2.4
                    }
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("I've looked")
                    role: "bodySmall"
                }
                TapHandler {
                    onTapped: Agents.prMergeLooked(dialog.merge.looked !== true)
                }
            }
        }
        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                id: cancelButton
                variant: "text"
                text: qsTr("Cancel")
                onClicked: Agents.prMergeClose()
            }
            // On the right, secondary: a click, or Tab then Enter.
            Button {
                variant: "tonal"
                iconPath: AgentGitGlyphs.merged
                text: qsTr("Merge")
                enabled: dialog.ready
                onActiveFocusChanged: Agents.prMergeFocus(activeFocus)
                onClicked: Agents.prMerge(mergeHover.hovered ? "click" : "key")
                HoverHandler {
                    id: mergeHover
                }
            }
        }
    }
}
