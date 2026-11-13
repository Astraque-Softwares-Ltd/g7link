# Motion

One motion language for G7 Link on the phone (Compose), G7 Link on the desktop (Qt Quick) and the
G7 Linux launcher (Qt Quick). The numbers live in `tokens.json` under `motion`; each surface reads
them through one module, which also applies reduced motion:

| Surface | Module |
|---|---|
| Android | `android/.../ui/theme/Motion.kt` (`G7Motion`) |
| Desktop | `Theme.qml`, from the generated `Tokens.qml` |
| Launcher | the `Motion` QML singleton in the g7linux repository |

Approved 2026-11-13.

## Tokens

| Token | Enter | Exit | For |
|---|---|---|---|
| `quick` | 120 ms | 90 ms | hover, press, color and selection changes, tooltips |
| `small` | 200 ms | 140 ms | chips, banners, toasts, menus, popovers, list rows coming and going |
| `large` | 260 ms | 180 ms | pages, tabs, sheets, expand and collapse, keys coming back after the keyboard |

Things leave faster than they arrive.

Curves, as cubic Béziers (Compose `CubicBezierEasing`, QML `Easing.BezierSpline`):

- **enter** `0.05, 0.7, 0.1, 1`: something arriving decelerates into place.
- **exit** `0.3, 0, 0.8, 0.15`: something leaving accelerates away.
- **move** `0.2, 0, 0, 1`: changes on screen such as color, position and indicators.
- **spring**, without bounce: only for motion that carries on from a gesture or can be
  interrupted, such as tab content, tab indicators, a toggle's knob, or letting go of a drag.

Distances:

- Elements rise 8 dp/px as they enter.
- Pages slide a tenth of their width, toward the side they come from.
- Popovers, menus and sheets scale from 0.96.
- Pages never scale.

There is no overshoot, except in moments designed with it: the star prompt, the Island's drop-in
and the Shelf's flick.

## What moves

**These get motion:**

- Transient UI coming and going.
- Moving between pages, tabs and views.
- Expanding and collapsing.
- A control's hover, press and selected states.
- Lists that change rarely: Home's notices, the notification feed, transfers.
- Determinate progress.

**These are always instant:**

- What input drives directly: the pointer, the touchpad, typing, scrolling, the text cursor, and a
  drag while the finger is down.
- Search results replacing as you type, and the launcher's keyboard selection. The glass glide
  stays.
- Summoning and closing the launcher. Its command views also switch instantly.
- UI leaving because the keyboard opened. Animating it would only push the typing field about.

The launcher's main window uses `quick` and `small` only; `large` is for its Settings window.

## Reduced motion

On **Android**, Remove animations (animator scale 0) makes every tween instant, and other animator
scales stretch them. `G7Motion` also swaps enter and exit transitions for none under
`LocalReducedMotion`, since some, like springs, aren't timed.

On the **desktop and in the launcher**, reduced motion comes on when the settings portal's
`reduced-motion` asks for it or when GNOME's animations are off. With it on, nothing moves, scales
or springs. Page changes keep a 120 ms crossfade, where an instant swap would lose your place;
everything else is instant. KDE's animation speed scales every duration.

## Performance

1. Animate opacity, scale and translation: `graphicsLayer` and the `AnimatedVisibility` and
   `AnimatedContent` transitions in Compose, and `opacity`, `scale`, `x`, `y` and `Translate` in
   QML. These don't lay anything out again.
2. Layout animates only where the layout change itself is unavoidable: expand and collapse, or the
   room the keyboard leaves. Then it takes `large` at most, and what it resizes must not allocate
   per frame.
3. Nothing allocates per frame. Cache brushes, paths and point arrays. Read animated values while
   measuring or drawing, not while composing.
4. Nothing animates where it can't be seen. Loops bind `running` to the item's and the window's
   visibility, or exist only while composed.
5. `layer.enabled` stays on only while an effect animates, unless it caches a static effect.
