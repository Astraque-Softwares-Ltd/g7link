// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// The reply box and the request dock: Ctrl+Enter never allows while the box
// has words; words deny only the request they were typed under, and never
// one that's held; Stop stays in reach while a request waits, as an icon;
// the agent's reason comes before its command, as its words, never markup.
// The mode always shows by its full name and opens the agent's modes, which
// number keys pick; a Deny that stops the turn says so.
Item {
    id: root
    width: 700
    height: 400

    AgentComposer {
        id: composer
        width: 600
        agentName: "Codex"
        agentId: "codex"
        folder: "g7linux"
        canSayWhy: true
        running: true
    }
    AgentDock {
        id: dock
        y: 150
        width: 600
        height: 200
        agentName: "Codex"
        folder: "g7linux"
    }
    SignalSpy {
        id: allowed
        target: composer
        signalName: "allow"
    }
    SignalSpy {
        id: denied
        target: composer
        signalName: "denySend"
    }
    SignalSpy {
        id: sent
        target: composer
        signalName: "send"
    }
    SignalSpy {
        id: stopped
        target: composer
        signalName: "stop"
    }
    SignalSpy {
        id: modeMenuAsked
        target: composer
        signalName: "modeMenuRequested"
    }
    AgentModeMenu {
        id: modeMenu
        modes: root.codexModes
    }
    SignalSpy {
        id: modePicked
        target: modeMenu
        signalName: "picked"
    }

    readonly property var codexModes: [
        { id: "ask", name: "Ask for approval", line: "Asks before working outside the folder or using the internet",
          on: true, why: "", checked: true, warn: false },
        { id: "auto", name: "Approve for me", line: "Only ask for actions detected as potentially unsafe",
          on: true, why: "", checked: false, warn: false },
        { id: "full", name: "Full access", line: "Unrestricted access to the internet and any file on your computer",
          on: false, why: "Not available now", checked: false, warn: true }
    ]
    function controls(mode, name, warn) {
        return { control: true, picker: { mode: mode, modeName: name, modeLine: "", modeWarn: warn,
                                           modes: root.codexModes, label: { model: "GPT X", effort: "" } } }
    }

    function request(token, held) {
        return { token: token, kind: "command", command: "cargo test", held: held === true, position: 1, total: 3 }
    }

    TestCase {
        name: "AgentComposer"
        when: windowShown

        function init() {
            composer.clear()
            composer.typedFor = ""
            composer.ask = root.request("a")
            composer.canSayWhy = true
            composer.running = true
            allowed.clear()
            denied.clear()
            sent.clear()
            stopped.clear()
            modeMenuAsked.clear()
            modePicked.clear()
            composer.controls = ({})
            composer.busy = false
            composer.focusInput()
        }

        function test_ctrl_enter_allows_only_while_the_box_is_empty() {
            keyClick(Qt.Key_Return, Qt.ControlModifier)
            compare(allowed.count, 1)
            keyClick(Qt.Key_X)
            keyClick(Qt.Key_Return, Qt.ControlModifier)
            compare(allowed.count, 1, "words in the box: Ctrl+Enter does nothing")
            compare(denied.count, 0)
            compare(composer.text, "x")
        }

        function test_enter_denies_with_the_words() {
            keyClick(Qt.Key_N)
            keyClick(Qt.Key_O)
            verify(composer.canDenySend)
            keyClick(Qt.Key_Return)
            compare(denied.count, 1)
            compare(denied.signalArguments[0][0], "a")
            compare(denied.signalArguments[0][1], "no")
            compare(composer.text, "", "sent words leave the box")
            compare(allowed.count, 0)
        }

        function test_words_are_for_the_request_they_were_typed_under() {
            keyClick(Qt.Key_N)
            composer.ask = root.request("b")
            verify(!composer.canDenySend, "the shown request changed under the words")
            keyClick(Qt.Key_Return)
            compare(denied.count, 0)
            keyClick(Qt.Key_O)
            verify(composer.canDenySend, "editing the words puts them under the new request")
        }

        function test_a_held_request_takes_no_words_and_no_shortcut() {
            composer.ask = root.request("a", true)
            keyClick(Qt.Key_Return, Qt.ControlModifier)
            compare(allowed.count, 0)
            keyClick(Qt.Key_N)
            verify(!composer.canDenySend)
            composer.ask = root.request("a", false)
            verify(composer.canDenySend)
        }

        function test_stop_stays_beside_send_while_a_request_waits() {
            const stop = findChild(composer, "stop")
            verify(stop.visible)
            composer.ask = ({})
            verify(stop.visible)
            composer.running = false
            verify(!stop.visible)
        }

        function test_stop_is_an_icon_that_stops_unless_already_stopping() {
            const stop = findChild(composer, "stop")
            compare(stop.text, "", "no word beside the icon")
            verify(stop.label.startsWith("Stop"), "its name is the tooltip")
            compare(stop.width, stop.height)
            mouseClick(stop)
            compare(stopped.count, 1)
            composer.busy = true
            mouseClick(stop)
            compare(stopped.count, 1, "a second press while stopping does nothing")
        }

        function test_the_reason_comes_first_and_is_never_markup() {
            dock.ask = { token: "r", kind: "command", command: "rm -rf build", position: 1, total: 1,
                         reason: "Clean <b>build</b> & rebuild\nthen test" }
            const reason = findChild(dock, "reason")
            const request = findChild(dock, "request")
            verify(reason.visible && request.visible)
            tryVerify(() => reason.y + reason.height <= request.y && request.y > 0, 1000, "the reason sits above the command")
            compare(dock.reasonMarkup, "<b>Codex says: </b>Clean &lt;b&gt;build&lt;/b&gt; &amp; rebuild<br>then test")
            dock.ask = root.request("a")
            verify(!reason.visible, "no reason, no line")
        }

        function test_the_mode_always_shows_by_its_full_name_and_opens_the_modes() {
            composer.ask = ({})
            composer.running = false
            composer.controls = root.controls("ask", "Ask for approval", false)
            const chip = findChild(composer, "mode")
            verify(chip.visible, "ask first shows too")
            compare(chip.Accessible.name, "Ask for approval")
            verify(!chip.warn)
            // After the model's name, once the footer is laid out.
            tryVerify(() => chip.x > findChild(composer, "model").x)
            mouseClick(chip)
            compare(modeMenuAsked.count, 1)
            composer.controls = root.controls("full", "Full access", true)
            verify(chip.warn, "nothing asked: the warning tint")
            compare(chip.ink, Theme.tintAmber)
            // While it works the mode rests until the turn ends.
            composer.running = true
            mouseClick(chip)
            compare(modeMenuAsked.count, 1)
            verify(chip.opacity < 1)
        }

        function test_number_keys_pick_a_mode_but_never_one_that_is_off() {
            modeMenu.openAt(10, 10, false, false)
            verify(modeMenu.opened)
            compare(modeMenu.current, 0, "on the one in use")
            keyClick(Qt.Key_3)
            verify(modeMenu.opened, "off: nothing happens")
            compare(modePicked.count, 0)
            keyClick(Qt.Key_2)
            verify(!modeMenu.opened)
            compare(modePicked.count, 1)
            compare(modePicked.signalArguments[0][0], "auto")
            modeMenu.openAt(10, 10, false, false)
            keyClick(Qt.Key_Escape)
            verify(!modeMenu.opened)
            compare(modePicked.count, 1)
        }

        function test_a_deny_that_stops_the_turn_says_so() {
            const note = findChild(dock, "denyNote")
            dock.ask = Object.assign(root.request("a"), { deny_stops: true })
            compare(note.text, "Deny stops this turn")
            dock.ask = root.request("a")
            verify(note.text !== "Deny stops this turn")
        }

        function test_without_say_why_the_box_waits_for_the_turn() {
            composer.canSayWhy = false
            verify(!composer.editable)
            composer.running = false
            verify(composer.editable)
            keyClick(Qt.Key_H)
            keyClick(Qt.Key_Return)
            compare(sent.count, 1)
            compare(sent.signalArguments[0][0], "h")
        }

        function test_the_dock_says_which_request_and_steps_back_for_words() {
            dock.ask = root.request("a")
            dock.composerHasText = false
            const allow = findChild(dock, "allow")
            const deny = findChild(dock, "deny")
            const keys = findChild(dock, "allowKeys")
            compare(findChild(dock, "count").text, "1 of 3")
            compare(allow.variant, "fill")
            verify(keys.visible && allow.enabled && deny.enabled)
            // Words in the box: Allow is an outline and its shortcut is hidden.
            dock.composerHasText = true
            compare(allow.variant, "outline")
            verify(!keys.visible)
            verify(allow.enabled, "Allow still works by click")
            // Held: both buttons wait, and it says why.
            dock.composerHasText = false
            dock.ask = root.request("a", true)
            verify(!allow.enabled && !deny.enabled && !keys.visible)
        }
    }
}
