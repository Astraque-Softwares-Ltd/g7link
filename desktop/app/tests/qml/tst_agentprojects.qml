// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import QtTest
import app.g7link

// The Agents list as projects, as the phone has it: what needs you first,
// then a group per folder with its threads and sessions, the busiest first.
TestCase {
    name: "AgentProjects"

    function thread(id, folder, state, updated, extra) {
        return Object.assign({ id: id, folder: folder, state: state, updated_ms: updated, pending: 0, asking: 0 }, extra || {})
    }
    function session(id, folder, state, updated, extra) {
        return Object.assign({ id: id, folder: folder, state: state, updated_ms: updated, guess: false }, extra || {})
    }
    function ids(items) {
        return items.map((i) => i.kind + ":" + i.id)
    }

    function test_sessions_sit_in_their_folder_beside_its_threads() {
        const groups = AgentProjects.groups([thread("t1", "g7linux", "done", 100)],
                                            [session("s1", "g7linux", "idle", 200), session("s2", "site", "idle", 50)])
        compare(groups.map((g) => g.folder), ["g7linux", "site"])
        compare(ids(groups[0].items), ["session:s1", "thread:t1"])
        compare(groups[0].threads, 1)
        compare(groups[0].sessions, 1)
    }

    function test_folders_order_by_needs_you_then_running_then_newest_then_name() {
        const groups = AgentProjects.groups([
            thread("a", "newest", "done", 900),
            thread("b", "running", "working", 100),
            thread("c", "asks", "done", 50, { pending: 1 }),
            thread("d", "older", "done", 300),
            thread("e", "alpha", "done", 300)
        ], [])
        compare(groups.map((g) => g.folder), ["asks", "running", "newest", "alpha", "older"])
    }

    function test_rows_rank_needs_you_may_running_pinned_then_newest() {
        const group = AgentProjects.groups([
            thread("rest", "f", "done", 900),
            thread("pinned", "f", "done", 10, { pinned: true }),
            thread("asking", "f", "working", 5, { asking: 1 }),
            thread("running", "f", "working", 20)
        ], [
            session("guess", "f", "needs_you", 1, { guess: true }),
            session("limited", "f", "limited", 1000)
        ])[0]
        compare(ids(group.items), ["thread:asking", "session:guess", "thread:running", "thread:pinned", "session:limited", "thread:rest"])
        compare(group.needsYou, 1)
        compare(group.mayNeedYou, 1)
        compare(group.running, 1)
    }

    function test_a_folder_shows_what_is_active_then_three_more() {
        const threads = [thread("w", "f", "working", 1)]
        for (let i = 0; i < 6; ++i)
            threads.push(thread("r" + i, "f", "done", 100 - i))
        const group = AgentProjects.groups(threads, [])[0]
        compare(group.firstShown, 4)
        compare(ids(AgentProjects.shown(group, false)), ["thread:w", "thread:r0", "thread:r1", "thread:r2"])
        compare(AgentProjects.shown(group, true).length, 7)
    }

    function test_needs_you_holds_requests_and_reported_waits_only_newest_first() {
        const items = AgentProjects.needsYou([
            thread("waiting", "f", "waiting", 10),
            thread("question", "f", "working", 30, { asking: 2 }),
            thread("done", "f", "done", 99)
        ], [
            session("said", "g", "needs_you", 20),
            session("guessed", "g", "needs_you", 40, { guess: true }),
            session("limited", "g", "limited", 50)
        ])
        compare(ids(items), ["thread:question", "session:said", "thread:waiting"])
    }

    function test_folders_start_open_when_active_or_newest_and_keep_the_users_choice() {
        const groups = AgentProjects.groups([
            thread("a", "busy", "working", 10),
            thread("b", "latest", "done", 500),
            thread("c", "quiet", "done", 5)
        ], [session("s", "maybe", "needs_you", 1, { guess: true })])
        const newest = AgentProjects.newest(groups)
        compare(newest, "latest")
        const open = (folder, chosen) => AgentProjects.isOpen(groups.find((g) => g.folder === folder), newest, chosen)
        verify(open("busy", []))
        verify(open("latest", []))
        verify(open("maybe", []))
        verify(!open("quiet", []))
        let chosen = AgentProjects.chose([], "busy", false)
        chosen = AgentProjects.chose(chosen, "quiet", true)
        verify(!open("busy", chosen))
        verify(open("quiet", chosen))
    }

    function test_choices_keep_the_newest_sixty_four() {
        let chosen = []
        for (let i = 0; i < 70; ++i)
            chosen = AgentProjects.chose(chosen, "f" + i, true)
        compare(chosen.length, AgentProjects.maxRemembered)
        compare(chosen[0][0], "f6")
        chosen = AgentProjects.chose(chosen, "f6", false)
        compare(chosen[chosen.length - 1][0], "f6")
        compare(chosen.length, AgentProjects.maxRemembered)
    }

    function strip(branch, extra) {
        return Object.assign({ repo: "ok", branch: branch, detached: "", worktree: false, changed: 0, pr: {} }, extra || {})
    }

    function test_a_folder_shows_its_main_strip_before_a_worktree() {
        const group = AgentProjects.groups([
            thread("wt", "f", "done", 900, { strip: strip("fix-tabs", { worktree: true }) }),
            thread("old", "f", "done", 50, { strip: strip("old-main") }),
            thread("main", "f", "done", 100, { strip: strip("main", { changed: 3, pr: { number: 42, state: "open" } }) }),
            thread("none", "f", "done", 999, { strip: {} })
        ], [])[0]
        compare(group.strip.branch, "main", "the newest outside a worktree")
        compare(group.strip.changed, 3)
        compare(group.strip.pr.number, 42)
        const worktreeOnly = AgentProjects.groups([thread("wt", "g", "done", 1, { strip: strip("fix-tabs", { worktree: true }) })], [])[0]
        compare(worktreeOnly.strip.branch, "fix-tabs", "a worktree's when it's the only one")
        const detached = AgentProjects.groups([thread("d", "h", "done", 1, { strip: strip("", { detached: "1a2b3c4" }) })], [])[0]
        verify(AgentProjects.stripShown(detached.strip))
        const none = AgentProjects.groups([thread("n", "i", "done", 1, { strip: {} }), session("s", "i", "idle", 2)], [])[0]
        verify(!AgentProjects.stripShown(none.strip))
    }

    function test_a_folder_shows_a_script_running_for_one_of_its_threads() {
        const group = AgentProjects.groups([
            thread("quiet", "f", "done", 900, { script: "" }),
            thread("dev", "f", "done", 100, { script: "dev" }),
            thread("busy", "f", "working", 50, { script: "test" })
        ], [])[0]
        compare(group.script, "test", "in the folder's order: the running thread first")
        compare(AgentProjects.groups([thread("a", "g", "done", 1)], [])[0].script, "")
    }

    function test_choices_are_stored_and_read_back_in_order() {
        let chosen = AgentProjects.chose([], "g7linux", false)
        chosen = AgentProjects.chose(chosen, "site", true)
        chosen = AgentProjects.chose(chosen, "g7linux", true)
        const stored = AgentProjects.encode(chosen)
        compare(stored, "1\tsite\n1\tg7linux")
        compare(AgentProjects.decode(stored), [["site", true], ["g7linux", true]])
        compare(AgentProjects.decode(""), [])
        compare(AgentProjects.decode(undefined), [])
    }

    function test_stored_choices_skip_long_names_newlines_and_bad_lines() {
        const tooLong = "x".repeat(AgentProjects.maxFolderChars + 1)
        const fits = "y".repeat(AgentProjects.maxFolderChars)
        const stored = AgentProjects.encode([[tooLong, true], ["two\nlines", false], ["cr\rhere", true], [fits, false], ["ok", true]])
        compare(stored, "0\t" + fits + "\n1\tok")
        compare(AgentProjects.decode("1\t" + tooLong + "\n2\tbad\nno tab\n0\n\n1\tok\r\n0\tok"), [["ok", false]],
                "a later line for the same folder wins")
        compare(AgentProjects.decode("0\t"), [["", false]], "the folder with no name")
    }

    function test_stored_choices_keep_the_newest_sixty_four() {
        let chosen = []
        for (let i = 0; i < 70; ++i)
            chosen.push(["f" + i, i % 2 === 0])
        const read = AgentProjects.decode(AgentProjects.encode(chosen))
        compare(read.length, AgentProjects.maxRemembered)
        compare(read[0], ["f6", true])
        compare(read[read.length - 1], ["f69", false])
        let lines = []
        for (let i = 0; i < 200; ++i)
            lines.push("1\tg" + i)
        const many = AgentProjects.decode(lines.join("\n"))
        compare(many.length, AgentProjects.maxRemembered)
        compare(many[many.length - 1][0], "g" + (AgentProjects.maxRemembered * 2 - 1), "a long store is read only so far")
    }

    function test_blank_folders_group_together() {
        const groups = AgentProjects.groups([thread("a", "", "done", 1), thread("b", "  ", "done", 2)], [])
        compare(groups.length, 1)
        compare(groups[0].folder, "")
    }
}
