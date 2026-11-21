// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick

// Source control's stroke icons on a 24×24 grid (round caps and joins),
// drawn by Icon: the boards' branch, pull request, merge, commit, arrows and
// pencil, the same paths as the phone's AgentGlyphs.kt. A worktree is
// Icons.folder.
// Lucide icons, ISC (portions from Feather, MIT): LICENSES/ISC-Lucide.txt.
QtObject {
    readonly property string branch: "M6 3v12M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9"
    readonly property string pullRequest: "M18 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM13 6h3a2 2 0 0 1 2 2v7M6 9v12"
    readonly property string merged: "M18 18a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21V9a9 9 0 0 0 9 9"
    readonly property string commit: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM3 12h6M15 12h6"
    readonly property string up: "M12 19V5M5 12l7-7 7 7"
    readonly property string down: "M12 5v14M19 12l-7 7-7-7"
    readonly property string pencil: "M21.17 6.81a1 1 0 0 0-3.98-3.98L3.84 16.17a2 2 0 0 0-.5.83l-1.32 4.35a.5.5 0 0 0 .62.62l4.35-1.32a2 2 0 0 0 .83-.5zM15 5l4 4"
    readonly property string passed: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM9 12l2 2 4-4"
    readonly property string failed: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM15 9l-6 6M9 9l6 6"
    readonly property string running: "M21 12a9 9 0 1 1-6.22-8.56"
    readonly property string waiting: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z"
}
