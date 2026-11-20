// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// The reply box and the request dock: Ctrl+Enter never allows while the box
// has words; words deny only the request they were typed under, and never
// one that's held; Stop stays in reach while a request waits.
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
