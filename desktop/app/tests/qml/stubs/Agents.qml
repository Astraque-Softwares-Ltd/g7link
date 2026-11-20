// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick

// Stands in for the Rust Agents in QML tests: it records what the page asked.
QtObject {
    property bool canSayWhy: true
    property bool busy: false
    property bool more: false
    property bool loading: false
    property var calls: []
    function record(call) { calls = calls.concat([call]) }
    function reset() { calls = [] }
    function toggle(key) { record("toggle " + key) }
    function toggleWrap(key) { record("wrap " + key) }
    function copyText(key) { record("copy " + key); return true }
    function typed() { record("typed") }
    function composerFocus(focused) { record("focus " + focused) }
    function answer(token, decision) { record("answer " + token + " " + decision) }
    function denyWithMessage(token, text) { record("deny " + token + " " + text) }
    function linkTarget(key, index) { return "" }
    function openLink(key, index) { record("open " + key + " " + index) }
}
