// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick
import app.g7link

// The colours of the agent views (plans/agent-colour.md §2): code by token
// class, tool glyphs, statuses, diffs and prose. G7's own themes read the
// generated tokens as they are; any other theme maps its named colours onto
// the same roles, each moved towards the ink until it reads at 4.5:1 on the
// worst ground it can meet. Nothing here is a typed hex.
QtObject {
    id: agentPalette

    readonly property var base: Tokens.data.themes.quiet.agents[Theme.dark ? "dark" : "light"]
    // G7's own colours: the fixed values pass without help.
    readonly property bool own: Theme.current === null

    // `c` moved towards the ink until it reads at `ratio` on `ground`.
    function legible(c, ground, ratio) {
        const ink = Theme.surfaceContent
        let out = c
        for (let step = 1; step <= 10 && Theme.contrast(out, ground) < ratio; ++step)
            out = Theme.mix(c, ink, step / 10)
        return out
    }
    function hex(c) {
        return String(c)
    }
    // A colour at `alpha` over `ground`, opaque.
    function over(ground, c, alpha) {
        return Qt.tint(ground, Qt.rgba(c.r, c.g, c.b, alpha))
    }

    // ---- Grounds ----
    readonly property color surface: own ? base.ground.surface : Theme.surface
    readonly property color codeGround: own ? base.ground.code : Theme.surfaceContainer
    readonly property color chipGround: own ? base.ground.chip : Theme.surfaceContainerHigh

    // ---- Steps between the tiers ----
    readonly property color ink: Theme.surfaceContent
    readonly property color muted: Theme.surfaceContentVariant
    readonly property color dim: own ? base.steps.dim : Theme.mix(muted, ink, 0.4)
    readonly property color lavenderInk: own ? base.steps.lavenderInk : Theme.mix(Theme.tintLavender, ink, 0.5)

    function text(c) {
        return legible(c, chipGround, 4.5)
    }

    // ---- Code, by token class (`#rrggbb` words for the bridge) ----
    readonly property var syntax: own ? base.syntax : ({
        plain: hex(ink),
        comment: hex(muted),
        keyword: hex(text(Theme.tintPink)),
        string: hex(text(Theme.tintTeal)),
        number: hex(text(Theme.tintCoral)),
        constant: hex(text(Theme.tintCoral)),
        type: hex(text(Theme.tintAmber)),
        function: hex(text(Theme.tintBlue)),
        variable: hex(text(lavenderInk)),
        operator: hex(text(dim)),
        punctuation: hex(muted),
        tag: hex(text(Theme.tintPink)),
        attribute: hex(text(Theme.tintAmber)),
        inserted: hex(text(Theme.tintGreen)),
        deleted: hex(text(Theme.tintRed)),
        dir: hex(muted),
        file: hex(ink),
        pos: hex(muted),
        error: hex(text(Theme.tintRed)),
        warn: hex(text(Theme.tintAmber)),
        ok: hex(text(Theme.tintGreen))
    })
    // "italic" or "semibold" by class.
    readonly property var styles: Tokens.data.themes.quiet.agents.styles

    // ---- Statuses ----
    readonly property color working: own ? base.status.working : text(Theme.tintBlue)
    readonly property color needsYou: own ? base.status.needsYou : Theme.tintAmber
    readonly property color onNeedsYou: own ? base.status.onNeedsYou : Theme.onColor(needsYou, surface, ink)
    readonly property color failed: own ? base.status.failed : Theme.tintRed
    readonly property color failedText: own ? base.status.failedText : text(Theme.tintRed)
    readonly property color done: own ? base.status.done : text(Theme.tintGreen)
    readonly property color paused: own ? base.status.paused : Theme.outline
    readonly property color inserted: syntax.inserted
    readonly property color deleted: syntax.deleted

    // A thread's or a session's state word as its colour.
    function stateColor(state) {
        switch (state) {
        case "waiting":
        case "needs_you": return needsYou
        case "working": return working
        case "done":
        case "idle": return done
        case "failed": return failed
        default: return paused
        }
    }

    // ---- Tools: the glyph by class, the trailing status ----
    function toolColor(cls) {
        if (own)
            return base.tool[cls] || base.tool.other
        switch (cls) {
        case "command": return text(Theme.tintTeal)
        case "edit": return text(Theme.tintCoral)
        case "read":
        case "web": return text(Theme.tintBlue)
        case "search": return text(Theme.tintPink)
        case "agent": return text(Theme.tintLavender)
        default: return muted
        }
    }
    readonly property color toolOk: own ? base.toolStatus.ok : done
    readonly property color toolFailed: own ? base.toolStatus.failed : failed
    readonly property color toolAsking: own ? base.toolStatus.asking : needsYou

    // ---- Diffs: plain text on washes, word marks as grounds ----
    function wash(spec, fallback) {
        const c = own ? Theme.rgb(spec.color) : fallback
        return over(codeGround, c, spec.alpha)
    }
    readonly property color addRow: wash(base.diff.addRow, Theme.tintGreen)
    readonly property color delRow: wash(base.diff.delRow, Theme.tintRed)
    readonly property color addWord: wash(base.diff.addWord, Theme.tintGreen)
    readonly property color delWord: wash(base.diff.delWord, Theme.tintRed)
    readonly property color diffText: own ? base.diff.text : ink
    readonly property color addSign: own ? base.diff.addSign : legible(Theme.tintGreen, addRow, 4.5)
    readonly property color delSign: own ? base.diff.delSign : legible(Theme.tintRed, delRow, 4.5)
    readonly property color lineNo: own ? base.diff.lineNo : dim
    readonly property color hunk: own ? base.diff.hunk : muted
    readonly property color hunkGround: own ? base.diff.hunkGround : Theme.surfaceContainerHigh

    // ---- Prose ----
    readonly property color inlineCode: own ? base.prose.inlineCode : text(Theme.tintCoral)
    readonly property color link: own ? base.prose.link : Theme.accent
    readonly property color headingRule: own ? base.prose.headingRule : Theme.tintLavender
    readonly property color doneBox: own ? base.prose.doneBox : done
    function alert(kind) {
        if (own) {
            switch (kind) {
            case "tip": return base.prose.alertTip
            case "important": return base.prose.alertImportant
            case "warning": return base.prose.alertWarning
            case "caution": return base.prose.alertCaution
            default: return base.prose.alertNote
            }
        }
        switch (kind) {
        case "tip": return text(Theme.tintTeal)
        case "important": return text(Theme.tintLavender)
        case "warning": return text(Theme.tintAmber)
        case "caution": return failedText
        default: return text(Theme.tintBlue)
        }
    }

    // What the bridge colours code and diffs with (JSON).
    readonly property string bridgeJson: {
        const colours = Object.assign({}, syntax)
        colours.inline = hex(inlineCode)
        colours.add = hex(inserted)
        colours.del = hex(deleted)
        colours.addWord = hex(addWord)
        colours.delWord = hex(delWord)
        colours.styles = styles
        return JSON.stringify(colours)
    }
}
