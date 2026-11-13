// SPDX-License-Identifier: GPL-3.0-or-later
pragma Singleton
import QtQuick
import app.g7link

// The active theme: G7's own colors (Tokens, generated from
// docs/design/tokens.json), G7 Linux's current theme, or a palette from
// G7 Linux's themes (Palettes), light or dark, resolved from Preferences and
// the system. Themes are colors only: every theme keeps the same layout,
// type and shapes. Material's "onX" roles are named "xContent" because QML
// reserves the "on" prefix.
QtObject {
    id: theme

    // The Mode setting: light or dark.
    readonly property bool modeDark: Preferences.colorMode === "dark"
                                     || (Preferences.colorMode === "system" && AppController.systemDark)
    readonly property bool reduceMotion: AppController.reduceMotion

    // ---- Palettes ----
    // G7 Linux's current themes, for "Match G7 Linux": { light, dark }, each
    // { name, dark, colors } (colors null: G7's own).
    readonly property var launcher: {
        if (AppController.launcherTheme === "")
            return null
        try {
            return JSON.parse(AppController.launcherTheme)
        } catch (e) {
            return null
        }
    }
    // The launcher's theme the Mode picks.
    readonly property var launcherNow: launcher ? launcher[modeDark ? "dark" : "light"] : null
    // What a theme shows now: null for G7's own colors, else { dark, colors }
    // with a palette's colors in the variant the Mode picks (a palette
    // without a light variant stays dark).
    function source(id) {
        if (id === "g7linux")
            return launcherNow && launcherNow.colors ? { dark: launcherNow.dark, colors: launcherNow.colors } : null
        const family = Palettes.families.find((f) => f.id === id)
        if (!family)
            return null
        const useDark = modeDark || !family.light
        return { dark: useDark, colors: useDark ? family.dark : family.light }
    }
    readonly property var current: source(Preferences.theme)
    readonly property bool dark: current ? current.dark : modeDark

    function rgb(hex) {
        const v = parseInt(hex.slice(1), 16)
        return Qt.rgba(((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255, 1)
    }
    function mix(a, b, t) {
        return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1)
    }
    function luminance(c) {
        const channel = (v) => v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
        return 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }
    function contrast(a, b) {
        const la = luminance(a)
        const lb = luminance(b)
        return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
    }
    // Text on a color: the ground or the ink, whichever reads better.
    function onColor(c, ground, ink) {
        return contrast(c, ground) >= contrast(c, ink) ? ground : ink
    }
    // A palette's colors on G7's roles.
    function mapped(c) {
        const ground = rgb(c.background)
        const ink = rgb(c.foreground)
        const accentColor = rgb(c.accent)
        const red = rgb(c.red)
        const raisedColor = mix(ground, ink, 0.06)
        const selectionColor = mix(ground, ink, 0.11)
        return {
            primary: ink,
            onPrimary: ground,
            primaryContainer: selectionColor,
            onPrimaryContainer: ink,
            secondaryContainer: selectionColor,
            onSecondaryContainer: ink,
            surface: ground,
            surfaceContainerLow: ground,
            surfaceContainer: raisedColor,
            surfaceContainerHigh: selectionColor,
            surfaceContainerHighest: mix(ground, ink, 0.14),
            onSurface: ink,
            onSurfaceVariant: mix(ground, ink, 0.62),
            outline: mix(ground, ink, 0.4),
            outlineVariant: rgb(c.border),
            accent: accentColor,
            onAccent: onColor(accentColor, ground, ink),
            warning: rgb(c.yellow),
            error: red,
            onError: onColor(red, ground, ink)
        }
    }
    readonly property var palette: current ? mapped(current.colors)
                                           : Tokens.data.themes.quiet.variants[dark ? "dark" : "light"]

    // A theme's preview, as the Mode shows it: its ground, its accent and
    // five of its tints.
    function preview(id) {
        const s = source(id)
        if (!s) {
            const v = Tokens.data.themes.quiet.variants[modeDark ? "dark" : "light"]
            const t = Tokens.data.themes.quiet.tints[modeDark ? "dark" : "light"]
            return { background: v.surface, accent: v.accent, dots: [t.blue, t.green, t.amber, t.red, t.pink] }
        }
        const c = s.colors
        return { background: c.background, accent: c.accent, dots: [c.blue, c.green, c.yellow, c.red, c.magenta] }
    }

    // ---- Color roles ----
    readonly property color primary: palette.primary
    readonly property color primaryContent: palette.onPrimary
    readonly property color primaryContainer: palette.primaryContainer
    readonly property color primaryContainerContent: palette.onPrimaryContainer
    readonly property color secondaryContainer: palette.secondaryContainer
    readonly property color secondaryContainerContent: palette.onSecondaryContainer
    readonly property color surface: palette.surface
    readonly property color surfaceContainerLow: palette.surfaceContainerLow
    readonly property color surfaceContainer: palette.surfaceContainer
    readonly property color surfaceContainerHigh: palette.surfaceContainerHigh
    readonly property color surfaceContainerHighest: palette.surfaceContainerHighest
    readonly property color surfaceContent: palette.onSurface
    readonly property color surfaceContentVariant: palette.onSurfaceVariant
    readonly property color outline: palette.outline
    readonly property color outlineVariant: palette.outlineVariant
    readonly property color warning: palette.warning
    readonly property color error: palette.error
    readonly property color errorContent: palette.onError
    // A quiet error surface (the "sharing your screen" pill): the error hue
    // over the surface, with the surface's own ink so text stays readable.
    readonly property color errorContainer: Qt.tint(surfaceContainerHigh, Qt.rgba(error.r, error.g, error.b, dark ? 0.22 : 0.14))
    readonly property color errorContainerContent: surfaceContent

    // The accent marks keyboard focus, the primary button and the G7 mark.
    readonly property color focusRing: palette.accent
    readonly property color accent: palette.accent
    readonly property color accentContent: palette.onAccent
    // Raised fields and keycaps, the selected or hovered row, and the 1 px lines.
    readonly property color raised: surfaceContainer
    readonly property color selection: surfaceContainerHigh
    // A mode that's on, or the item shown: the accent, faint, with an accent
    // edge, so it never reads as hover.
    readonly property color activeFill: Qt.rgba(accent.r, accent.g, accent.b, dark ? 0.20 : 0.14)
    readonly property color line: outlineVariant

    // ---- Glyph tints ----
    // Each section and action draws its bare glyph in one tint.
    readonly property var tints: current
        ? {
            lavender: current.colors.purple,
            blue: current.colors.blue,
            teal: current.colors.cyan,
            coral: current.colors.orange,
            pink: current.colors.magenta,
            amber: current.colors.yellow,
            green: current.colors.green,
            red: current.colors.red
        }
        : Tokens.data.themes.quiet.tints[dark ? "dark" : "light"]
    function tint(name) {
        return tints[name] || surfaceContentVariant
    }
    readonly property color tintLavender: tint("lavender")
    readonly property color tintBlue: tint("blue")
    readonly property color tintTeal: tint("teal")
    readonly property color tintCoral: tint("coral")
    readonly property color tintPink: tint("pink")
    readonly property color tintAmber: tint("amber")
    readonly property color tintGreen: tint("green")
    readonly property color tintRed: tint("red")
    // A tint washed into the ground: avatar and tile backgrounds.
    function wash(c) { return Qt.tint(surface, Qt.rgba(c.r, c.g, c.b, dark ? 0.16 : 0.14)) }
    // Battery: green when it's fine, amber when low, red when nearly empty.
    function batteryColor(level) {
        if (level < 0)
            return surfaceContentVariant
        return level <= 10 ? tintRed : (level <= 20 ? tintAmber : tintGreen)
    }
    readonly property color scrim: Qt.rgba(0, 0, 0, dark ? 0.5 : 0.32)

    // ---- Shape ----
    readonly property var shape: Tokens.data.themes.quiet.shape
    readonly property int radiusXs: shape.xs
    readonly property int radiusSm: shape.sm
    readonly property int radiusMd: shape.md
    readonly property int radiusLg: shape.lg
    readonly property int radiusXl: shape.xl
    // Segmented controls and other once-pill shapes.
    readonly property int radiusFull: shape.full

    // ---- Type ----
    // Bundled with the app (assets/fonts).
    readonly property string fontUi: "Geist Mono"
    readonly property string fontMono: "Geist Mono"
    readonly property var typography: Tokens.data.themes.quiet.typography

    // ---- Layout and spacing ----
    readonly property var layout: Tokens.data.layout.desktop
    readonly property var frame: Tokens.data.themes.quiet.layout
    readonly property int railWidth: layout.railWidth
    readonly property int topBarHeight: frame.headerHeight
    readonly property int headerHeight: frame.headerHeight
    readonly property int sidebarWidth: frame.sidebarWidth
    readonly property int footerHeight: frame.footerHeight
    readonly property int rowHeight: frame.rowHeight
    readonly property int rowPadding: frame.rowPadding
    readonly property int rowRadius: frame.rowRadius
    readonly property int columnWidth: frame.columnWidth
    readonly property int contentPadding: layout.contentPadding
    readonly property int gutter: layout.gutter
    function space(step) { return Tokens.data.spacing.scale[step] }

    // Focus rings show for keyboard focus only, not after a click (TapHandler
    // doesn't take activeFocus on click; activeFocus on a control comes from Tab).
    function focusVisible(item) {
        return item !== null && item !== undefined && item.activeFocus
            && (item.focusReason === undefined
                || item.focusReason === Qt.TabFocusReason
                || item.focusReason === Qt.BacktabFocusReason)
    }

    // ---- Motion ----
    // QML springs take a strength and a damping ratio; these are tuned to
    // match the token springs (gentle / standard / snappy).
    readonly property real springGentle: 2.4
    readonly property real dampingGentle: 0.36
    readonly property real springStandard: 3.6
    readonly property real dampingStandard: 0.32
    readonly property real springSnappy: 5.6
    readonly property real dampingSnappy: 0.34
    // Durations and curves from the shared motion spec (docs/design/motion.md), already
    // scaled by KDE's animation speed. Under reduced motion they're 0, so nothing moves;
    // only `pageFade` keeps a short crossfade where an instant swap would lose your place.
    readonly property real motionFactor: reduceMotion ? 0 : Math.max(0, AppController.motionFactor)
    function motionMs(ms) { return Math.round(ms * motionFactor) }
    readonly property int quickIn: motionMs(Tokens.data.motion.duration.quick.enter)
    readonly property int quickOut: motionMs(Tokens.data.motion.duration.quick.exit)
    readonly property int smallIn: motionMs(Tokens.data.motion.duration.small.enter)
    readonly property int smallOut: motionMs(Tokens.data.motion.duration.small.exit)
    readonly property int largeIn: motionMs(Tokens.data.motion.duration.large.enter)
    readonly property int largeOut: motionMs(Tokens.data.motion.duration.large.exit)
    readonly property int pageFade: reduceMotion ? Tokens.data.motion.duration.quick.enter : largeIn
    // For `easing.type: Easing.BezierSpline; easing.bezierCurve: Theme.easeEnter`.
    readonly property var easeEnter: Tokens.data.motion.easing.enter.concat([1, 1])
    readonly property var easeExit: Tokens.data.motion.easing.exit.concat([1, 1])
    readonly property var easeMove: Tokens.data.motion.easing.move.concat([1, 1])
    // How far things rise as they enter, and the scale popovers and sheets grow from.
    readonly property real rise: reduceMotion ? 0 : Tokens.data.motion.distance.rise
    readonly property real popScale: reduceMotion ? 1 : Tokens.data.motion.distance.popScale
    // Older names for the quick and small fades.
    readonly property int fadeFast: quickIn
    readonly property int fadeNormal: smallIn
    readonly property real hoverLift: reduceMotion ? 0 : Tokens.data.motion.hoverLift
    readonly property real pressScale: reduceMotion ? 1 : Tokens.data.motion.pressScale
}
