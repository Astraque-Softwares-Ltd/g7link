// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// Code keeps every character in its own cell. Geist Mono's standard
// ligatures draw `--` (and `->`, `==` …) as one cell-wide glyph over the space
// before it, so "pnpm exec tsc --noEmit" showed as "tsc--noEmit".
Item {
    id: root
    width: 900
    height: 200

    // The app compiles the font in; here it's loaded from the repository.
    FontLoader {
        id: mono
        source: Qt.resolvedUrl("../../../../assets/fonts/GeistMono-Variable.ttf")
    }

    Txt {
        id: code
        role: "code"
    }
    // The same number of characters, none that could join.
    Txt {
        id: plain
        role: "code"
    }
    Txt {
        id: wrapped
        width: 400
        role: "code"
        wrapMode: Text.WrapAnywhere
        elide: Text.ElideNone
    }

    TestCase {
        name: "CodeText"
        when: windowShown

        function initTestCase() {
            tryCompare(mono, "status", FontLoader.Ready)
            compare(mono.name, Theme.fontMono)
        }

        function test_every_character_keeps_its_cell_data() {
            return [
                { tag: "--", text: "pnpm exec tsc --noEmit" },
                { tag: "-- again", text: "rg --files" },
                { tag: "->", text: "a -> b" },
                { tag: "==", text: "x == y" },
                { tag: "!=", text: "x != y" },
                { tag: ">=", text: "x >= 1" },
                { tag: "...", text: "wait..." }
            ]
        }

        function test_every_character_keeps_its_cell(data) {
            code.text = data.text
            plain.text = "x".repeat(data.text.length)
            wrapped.text = data.text
            waitForRendering(code)
            compare(code.implicitWidth, plain.implicitWidth)
            compare(wrapped.contentWidth, plain.implicitWidth)
        }
    }
}
