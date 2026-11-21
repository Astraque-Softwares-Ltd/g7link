// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import app.g7link

// Open a pull request (§SC9.2): the title (the thread's offered), the
// description, the branch it goes into (the repository's default when
// left empty), Draft, and "Ask <agent> to write it". When the branch
// isn't pushed the button reads "Push, then open": two jobs, each shown.
Sheet {
    id: dialog
    // Agents.git's form: { open, title: { set, asking, offer }, body: { … } }.
    property var form: ({})
    property string agentName
    property string defaultBranch
    property bool pushed: false
    // The open or push job runs.
    property bool busy: false
    // Words for a refusal, from the panel ("" for none).
    property string problem
    property bool canAsk: true

    readonly property bool shown: form.open === true
    readonly property var title: form.title || ({})
    readonly property var body: form.body || ({})
    property bool draft: false
    property int titleN: -1
    property int bodyN: -1
    property bool setting: false

    function take(field, input, seen) {
        const set = field.set || {}
        if ((set.n ?? 0) === seen)
            return seen
        setting = true
        input.text = set.text || ""
        setting = false
        return set.n ?? 0
    }
    onTitleChanged: titleN = take(title, titleField, titleN)
    onBodyChanged: bodyN = take(body, bodyInput, bodyN)

    cardWidth: 560
    dismissable: !busy
    onShownChanged: {
        if (shown) {
            open()
            Qt.callLater(() => titleField.input.forceActiveFocus(Qt.OtherFocusReason))
        } else {
            close()
        }
    }
    onDismissed: Agents.prForm(false)

    Column {
        width: parent.width
        spacing: 12
        Txt {
            text: qsTr("Open pull request")
            role: "title"
        }
        Column {
            width: parent.width
            spacing: 4
            Txt {
                text: qsTr("Title")
                role: "label"
                muted: true
            }
            Field {
                id: titleField
                width: parent.width
                readOnly: dialog.title.asking === true
                onTextChanged: if (!dialog.setting) Agents.gitField("title", text)
            }
        }
        Column {
            width: parent.width
            spacing: 4
            Txt {
                text: qsTr("Description")
                role: "label"
                muted: true
            }
            Rectangle {
                width: parent.width
                height: 140
                radius: Theme.radiusSm
                color: Theme.raised
                border.width: 1
                border.color: bodyInput.activeFocus ? Theme.focusRing : Theme.outlineVariant
                Flickable {
                    id: bodyScroll
                    anchors.fill: parent
                    anchors.margins: 8
                    contentHeight: bodyInput.contentHeight
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    TextEdit {
                        id: bodyInput
                        width: bodyScroll.width
                        wrapMode: TextEdit.Wrap
                        textFormat: TextEdit.PlainText
                        font.family: Theme.fontUi
                        font.pixelSize: Theme.typography.bodySmall.size
                        color: Theme.surfaceContent
                        selectionColor: Theme.selection
                        selectedTextColor: Theme.surfaceContent
                        readOnly: dialog.body.asking === true
                        Accessible.name: qsTr("Description")
                        onTextChanged: if (!dialog.setting) Agents.gitField("body", text)
                        Txt {
                            visible: bodyInput.text.length === 0 && dialog.body.asking === true
                            text: qsTr("Asking %1…").arg(dialog.agentName)
                            role: "bodySmall"
                            muted: true
                        }
                    }
                }
            }
        }
        Row {
            width: parent.width
            spacing: 12
            Column {
                spacing: 4
                Txt {
                    text: qsTr("Into")
                    role: "label"
                    muted: true
                }
                Field {
                    id: baseField
                    width: 200
                    placeholder: dialog.defaultBranch
                }
            }
            Row {
                anchors.bottom: parent.bottom
                spacing: 8
                height: baseField.height
                Toggle {
                    anchors.verticalCenter: parent.verticalCenter
                    label: qsTr("Draft")
                    checked: dialog.draft
                    onToggled: (on) => dialog.draft = on
                }
                Txt {
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Draft")
                    role: "bodySmall"
                }
            }
        }
        Flow {
            width: parent.width
            spacing: 2
            TextAction {
                visible: dialog.title.offer === true
                text: qsTr("Use %1's message").arg(dialog.agentName)
                onClicked: Agents.gitUseOffer("pr")
            }
            TextAction {
                visible: dialog.title.asking !== true
                enabled: dialog.canAsk
                iconPath: AgentGitGlyphs.pencil
                iconColor: Theme.surfaceContentVariant
                text: qsTr("Ask %1 to write it").arg(dialog.agentName)
                onClicked: Agents.gitAsk("pr")
            }
        }
        Txt {
            visible: dialog.problem.length > 0
            width: parent.width
            text: dialog.problem
            role: "bodySmall"
            color: AgentPalette.failedText
            wrapMode: Text.WordWrap
        }
        Row {
            anchors.right: parent.right
            spacing: 10
            Button {
                variant: "text"
                text: qsTr("Cancel")
                enabled: !dialog.busy
                onClicked: Agents.prForm(false)
            }
            Button {
                text: dialog.busy ? qsTr("Opening…") : dialog.pushed ? qsTr("Open pull request") : qsTr("Push, then open")
                busy: dialog.busy
                enabled: titleField.text.trim().length > 0 && dialog.title.asking !== true
                onClicked: Agents.prCreate(titleField.text, bodyInput.text, baseField.text.trim(), dialog.draft)
            }
        }
    }
}
