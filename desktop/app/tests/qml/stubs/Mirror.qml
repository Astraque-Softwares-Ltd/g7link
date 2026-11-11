// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick

// Stands in for the Rust Mirror in QML tests: the windows are what a test
// sets, and what a window asks for is kept in `asked` (`[name, key, ...]`).
QtObject {
    property string windows: "[]"
    property bool muted: false
    property var asked: []
    // The stub VideoView of each window, by stream, so a test can send pictures.
    property var views: ({})

    function note(name, args) { asked.push([name].concat(args)) }
    function count(name) { return asked.filter((a) => a[0] === name).length }
    function last(name) {
        const all = asked.filter((a) => a[0] === name)
        return all.length > 0 ? all[all.length - 1] : null
    }

    function saveGeometry(key, x, y, width, height) { note("saveGeometry", [key, width, height]) }
    function resize(key, width, height) { note("resize", [key, width, height]) }
    function stop(key) { note("stop", [key]) }
    function reopen(key) { note("reopen", [key]) }
    function syncClipboardOnFocus(key) {}
    function enterWindow(key) {}
    function leaveWindow() {}
    function requestControl(key) { note("requestControl", [key]) }
    function press(key, name) { note("press", [key, name]) }
    function touch(key, action, x, y) { note("touch", [key, action]) }
    function scroll(key, x, y, dx, dy) {}
    function text(key, text) {}
    function paste(key) {}
    function dropFiles(urls) {}
    function toggleSound() {}
    function screenshotClipboard(key) {}
    function screenshotFile(key) { note("screenshotFile", [key]) }
    function toggleRecording(key) { note("toggleRecording", [key]) }
    function setStayAwake(key, on) {}
    function setScreenOff(key, on) {}
}
