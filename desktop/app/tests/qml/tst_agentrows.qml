// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// The Agents timeline's rows: their words come from structured data, agent
// text only ever reaches a Text as markup the app built (StyledText) or as
// plain text, links go through the page, and code keeps its cells.
Item {
    id: root
    width: 700
    height: 400

    FontLoader {
        id: mono
        source: Qt.resolvedUrl("../../../../assets/fonts/GeistMono-Variable.ttf")
    }

    Component {
        id: toolLine
        AgentToolLine {}
    }
    Component {
        id: groupLine
        AgentGroupLine {}
    }
    Component {
        id: foldLine
        AgentFoldLine {}
    }
    Component {
        id: markLine
        AgentMarkLine {}
    }
    Component {
        id: blockRow
        AgentBlock {}
    }
    Component {
        id: codeRow
        AgentCode {}
    }
    Component {
        id: liveLine
        AgentLiveLine {}
    }

    TestCase {
        name: "AgentRows"
        when: windowShown

        function tool(cls, object, status, more) {
            return createTemporaryObject(toolLine, root, {
                width: 600, key: "i:t1", depth: 0, label: JSON.stringify({ class: cls, object: object, more: more || 0 }),
                status: status, hasDetail: true, hasOutput: false, isOpen: false, adds: 4, dels: 2, ms: 38000
            })
        }

        function init() {
            Agents.reset()
        }

        function test_a_tool_line_says_what_it_did_data() {
            return [
                { tag: "ran", cls: "command", status: "ok", line: "Ran cargo test" },
                { tag: "running", cls: "command", status: "running", line: "Running cargo test" },
                { tag: "asking", cls: "command", status: "asking", line: "Wants to run cargo test" },
                { tag: "failed", cls: "command", status: "failed", line: "Failed: cargo test" },
                { tag: "stopped", cls: "command", status: "stopped", line: "Stopped: cargo test" },
                { tag: "edited", cls: "edit", status: "ok", line: "Edited cargo test" },
                { tag: "denied edit", cls: "edit", status: "denied", line: "Denied: edit cargo test" },
                { tag: "asking edit", cls: "edit", status: "asking", line: "Wants to edit cargo test" },
                { tag: "web", cls: "web", status: "ok", line: "Searched the web: cargo test" },
                { tag: "other", cls: "mcp", status: "ok", line: "cargo test" }
            ]
        }

        function test_a_tool_line_says_what_it_did(data) {
            const line = tool(data.cls, "cargo test", data.status)
            compare(line.Accessible.name, data.line)
        }

        function test_more_files_follow_the_object() {
            const line = tool("edit", "src/a.rs", "ok", 2)
            compare(line.Accessible.name, "Edited src/a.rs and 2 more")
        }

        function test_a_tool_line_opens_by_its_key() {
            const line = tool("command", "ls", "ok")
            mouseClick(line)
            compare(Agents.calls, ["toggle i:t1"])
        }

        function test_a_group_line_sums_up() {
            const summary = { first: { class: "command", count: 3 }, second: { class: "edit", count: 2 }, others: 0 }
            const group = createTemporaryObject(groupLine, root, {
                key: "g:t1", depth: 0, summary: JSON.stringify(summary), count: 5, failed: true, isOpen: false
            })
            // Without a translation loaded, plural forms keep their "(s)".
            compare(group.line, "Ran 3 command(s) and changed 2 file(s)")
            summary.others = 4
            group.summary = JSON.stringify(summary)
            compare(group.line, "Ran 3 command(s), changed 2 file(s) and 4 other action(s)")
        }

        function test_the_fold_says_how_long_it_worked_data() {
            return [
                { tag: "seconds", ms: 12000, line: "Worked for 12s" },
                { tag: "minutes", ms: 134000, line: "Worked for 2m 14s" },
                { tag: "hours", ms: 3780000, line: "Worked for 1h 03m" }
            ]
        }

        function test_the_fold_says_how_long_it_worked(data) {
            const fold = createTemporaryObject(foldLine, root, {
                key: "f:1", depth: 0, workedMs: data.ms, hidden: 4, isOpen: false
            })
            compare(fold.Accessible.name, data.line)
        }

        function test_a_mark_says_who_answered_data() {
            return [
                { tag: "waiting", kind: "waiting", by: "", line: "Waiting for you" },
                { tag: "held", kind: "held", by: "", line: "Waiting for you" },
                { tag: "phone", kind: "allowed", by: "Pixel 8 Pro", line: "Allowed on Pixel 8 Pro" },
                { tag: "denied", kind: "denied", by: "Pixel 8 Pro", line: "Denied on Pixel 8 Pro" },
                { tag: "rule", kind: "rule", by: "", line: "Allowed by your rule for this thread" },
                { tag: "expired", kind: "expired", by: "", line: "Expired" }
            ]
        }

        function test_a_mark_says_who_answered(data) {
            const mark = createTemporaryObject(markLine, root, { depth: 0, markKind: data.kind, by: data.by })
            compare(mark.line, data.line)
        }

        function test_agent_text_is_styled_text_and_links_go_through_the_page() {
            const block = createTemporaryObject(blockRow, root, {
                key: "b:m1:0", itemId: "m1", depth: 0,
                styled: "See <a href=\"0\">the checklist</a> and &lt;b&gt;", blockKind: "paragraph",
                level: 0, quote: 0, marker: "", markerNumber: 0, last: true, phase: "final",
                streaming: false, whenMs: 0, truncated: true
            })
            const text = findChild(block, "text")
            verify(text)
            compare(text.textFormat, Text.StyledText)
            verify(text.text.indexOf("&lt;b&gt;") >= 0)
            const spy = createTemporaryObject(signalSpy, root, { target: block, signalName: "linkActivated" })
            text.linkActivated("0")
            compare(spy.count, 1)
            compare(spy.signalArguments[0][0], "b:m1:0")
            compare(spy.signalArguments[0][1], 0)
        }

        function test_code_keeps_its_cells_and_copies_its_source() {
            tryCompare(mono, "status", FontLoader.Ready)
            const code = createTemporaryObject(codeRow, root, {
                key: "b:m1:1", depth: 0, quote: 0, lang: "sh", wrapped: false, lines: 1,
                styled: "pnpm&nbsp;exec&nbsp;tsc&nbsp;--noEmit"
            })
            const source = findChild(code, "source")
            compare(source.textFormat, Text.StyledText)
            compare(source.font.family, Theme.fontMono)
            compare(source.font.features.liga, 0)
            compare(source.font.features.calt, 0)
        }

        function test_the_live_line_stands_still_when_asked() {
            const live = createTemporaryObject(liveLine, root, {
                liveLabel: "thinking", text: "", sinceMs: Date.now() - 108000, animate: false
            })
            compare(live.label, "Thinking")
            compare(live.clock(live.sinceMs).slice(0, 2), "1:")
        }
    }

    Component {
        id: signalSpy
        SignalSpy {}
    }
}
