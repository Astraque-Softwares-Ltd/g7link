// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick

// The Agents list as projects, as the phone shows it (AgentProjects.kt): what
// needs you first, then a group per folder holding its threads and the
// sessions started in it, the busiest first, so a session today is never
// below last month's threads. Rows are Agents.threads and Agents.sessions as
// the page parses them; an item is { kind: "thread" | "session", id, row }.
QtObject {
    // Rows a folder shows past the ones that need you or run, before "n more".
    readonly property int restShown: 3
    // Folders whose open or closed state is kept.
    readonly property int maxRemembered: 64
    // The PC sends a folder's name in at most 64 characters (§2.2); a longer one isn't kept.
    readonly property int maxFolderChars: 64

    // Where a row sits in its folder, first to last.
    readonly property int needsYouRank: 0
    readonly property int mayNeedYouRank: 1
    readonly property int runningRank: 2
    readonly property int pinnedRank: 3
    readonly property int restRank: 4

    // A request or a question waits on it.
    function threadNeedsYou(t) {
        return t.state === "waiting" || (t.pending || 0) > 0 || (t.asking || 0) > 0
    }
    // Its agent reported it waits for you; a guess never counts.
    function sessionNeedsYou(s) {
        return s.state === "needs_you" && s.guess !== true
    }
    // It looks like it waits, guessed from its file: it keeps its folder open, uncounted.
    function sessionMayNeedYou(s) {
        return s.state === "needs_you_guess" || (s.state === "needs_you" && s.guess === true)
    }
    function rank(item) {
        const r = item.row
        if (item.kind === "session") {
            if (r.state === "limited")
                return restRank
            if (sessionNeedsYou(r))
                return needsYouRank
            if (sessionMayNeedYou(r))
                return mayNeedYouRank
            return r.state === "working" ? runningRank : restRank
        }
        if (threadNeedsYou(r))
            return needsYouRank
        if (r.state === "working")
            return runningRank
        return r.pinned === true ? pinnedRank : restRank
    }

    function items(threads, sessions) {
        return threads.map((t) => ({ kind: "thread", id: t.id, row: t }))
            .concat(sessions.map((s) => ({ kind: "session", id: s.id, row: s })))
    }
    function key(item) {
        return item.kind + ":" + item.id
    }
    function updated(item) {
        return item.row.updated_ms || 0
    }
    function byText(a, b) {
        return a < b ? -1 : (a > b ? 1 : 0)
    }
    function flag(on) {
        return on ? 1 : 0
    }

    // The folders: something that needs you first, then something running,
    // then the most recently changed, then by name.
    function groups(threads, sessions) {
        const byFolder = new Map()
        for (const item of items(threads, sessions)) {
            const folder = (item.row.folder || "").trim()
            if (!byFolder.has(folder))
                byFolder.set(folder, [])
            byFolder.get(folder).push(item)
        }
        const out = []
        for (const [folder, rows] of byFolder)
            out.push(group(folder, rows))
        out.sort((a, b) => flag(b.needsYou > 0) - flag(a.needsYou > 0)
                 || flag(b.running > 0) - flag(a.running > 0)
                 || b.lastMs - a.lastMs
                 || byText(a.folder, b.folder))
        return out
    }

    // One folder: its rows by rank, then the newest; its counts; its strip and running script.
    function group(folder, rows) {
        const ordered = rows.slice().sort((a, b) => rank(a) - rank(b) || updated(b) - updated(a) || byText(key(a), key(b)))
        let needsYou = 0
        let mayNeedYou = 0
        let running = 0
        let threads = 0
        let lastMs = 0
        for (const item of ordered) {
            const r = rank(item)
            if (r === needsYouRank)
                needsYou++
            else if (r === mayNeedYouRank)
                mayNeedYou++
            else if (r === runningRank)
                running++
            if (item.kind === "thread")
                threads++
            lastMs = Math.max(lastMs, updated(item))
        }
        const active = needsYou + mayNeedYou + running
        return {
            folder: folder,
            items: ordered,
            needsYou: needsYou,
            mayNeedYou: mayNeedYou,
            running: running,
            threads: threads,
            sessions: ordered.length - threads,
            lastMs: lastMs,
            strip: stripOf(ordered),
            script: scriptOf(ordered),
            // Rows shown before "n more": every one that needs you, may, or runs, then restShown more.
            firstShown: Math.min(ordered.length, active + restShown)
        }
    }

    // A thread's strip ({ branch, detached, worktree, changed, pr }) says where it is.
    function stripShown(strip) {
        return !!strip && ((strip.branch || "").length > 0 || (strip.detached || "").length > 0)
    }
    // The strip of its newest thread that has one, preferring one outside a worktree ({} for none).
    function stripOf(ordered) {
        const withStrip = ordered.filter((i) => i.kind === "thread" && stripShown(i.row.strip))
            .sort((a, b) => updated(b) - updated(a))
        const pick = withStrip.find((i) => i.row.strip.worktree !== true) || withStrip[0]
        return pick ? pick.row.strip : ({})
    }
    // A script running for one of its threads, by name, the first in order ("" for none).
    function scriptOf(ordered) {
        const pick = ordered.find((i) => i.kind === "thread" && (i.row.script || "").length > 0)
        return pick ? pick.row.script : ""
    }

    // What needs you: threads with a request or question, and sessions whose
    // agent said it waits; the newest first.
    function needsYou(threads, sessions) {
        return items(threads.filter((t) => threadNeedsYou(t)), sessions.filter((s) => sessionNeedsYou(s)))
            .sort((a, b) => updated(b) - updated(a) || byText(key(a), key(b)))
    }

    // The folder that changed last, which starts open ("" for none).
    function newest(groups) {
        let best = null
        for (const g of groups) {
            if (best === null || g.lastMs > best.lastMs)
                best = g
        }
        return best === null ? "" : best.folder
    }

    // Whether a folder is open: as the user left it, else when something in it
    // needs you, may, or runs, or it changed last. `chosen` is
    // [[folder, open], ...], the oldest choice first.
    function isOpen(group, newestFolder, chosen) {
        const choice = chosen.find((c) => c[0] === group.folder)
        if (choice !== undefined)
            return choice[1]
        return group.needsYou > 0 || group.mayNeedYou > 0 || group.running > 0 || group.folder === newestFolder
    }

    // The user opened or closed `folder`: kept last, the oldest choices dropped past maxRemembered.
    function chose(chosen, folder, open) {
        const next = chosen.filter((c) => c[0] !== folder)
        next.push([folder, open])
        return next.slice(Math.max(0, next.length - maxRemembered))
    }

    function storable(folder) {
        return folder.length <= maxFolderChars && folder.indexOf("\n") < 0 && folder.indexOf("\r") < 0
    }

    // The choices as one stored string (Preferences.agentFolders): a line per
    // folder, "1" or "0" and a tab before its name, the oldest first.
    function encode(chosen) {
        const kept = chosen.filter((c) => storable(c[0]))
        return kept.slice(Math.max(0, kept.length - maxRemembered))
            .map((c) => (c[1] ? "1\t" : "0\t") + c[0]).join("\n")
    }

    // Reads encode's string back as [[folder, open], ...]; anything else is skipped.
    function decode(stored) {
        if (!stored)
            return []
        let out = []
        for (const line of stored.split(/\r\n|\r|\n/).slice(0, maxRemembered * 2)) {
            if (line.length < 2 || line[1] !== "\t" || (line[0] !== "1" && line[0] !== "0"))
                continue
            const folder = line.substring(2)
            if (!storable(folder))
                continue
            out = out.filter((c) => c[0] !== folder)
            out.push([folder, line[0] === "1"])
        }
        return out.slice(Math.max(0, out.length - maxRemembered))
    }

    // The rows a folder shows: all of them, or the first ones before "n more".
    function shown(group, all) {
        return all ? group.items : group.items.slice(0, group.firstShown)
    }
}
