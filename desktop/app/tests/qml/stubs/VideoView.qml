// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import app.g7link

// Stands in for the C++ VideoView in QML tests: a test sets `frameSize`, as
// a picture arriving would.
Item {
    property string stream
    property size frameSize: Qt.size(0, 0)
    property real frames: 0
    property string windowIcon
    property bool acceptsText: false
    signal textCommitted(string text)

    function pictureRect() {
        if (frameSize.width <= 0 || frameSize.height <= 0)
            return Qt.rect(0, 0, 0, 0)
        const scale = Math.min(width / frameSize.width, height / frameSize.height)
        return Qt.rect((width - frameSize.width * scale) / 2, (height - frameSize.height * scale) / 2,
                       frameSize.width * scale, frameSize.height * scale)
    }

    onStreamChanged: Mirror.views[stream] = this
}
