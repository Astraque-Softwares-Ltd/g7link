// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick

// Stands in for the Rust Preferences in QML tests: G7, light.
QtObject {
    property string theme: "quiet"
    property string colorMode: "light"
    property bool enterSends: false
    property bool starPromptDone: false
    property bool starPromptUpdated: false
    property int starPromptMinutes: 0
    property bool quietWhenLocked: false
    property int uiScale: 100
    property string commandPaletteHotkey: ""
    property bool streamPcAudioToPhone: false
    property bool g7Island: true
    property bool g7IslandCalls: true
    property bool g7IslandMedia: true
    property bool g7IslandNotifications: true
    property bool g7IslandTransfers: true
}
