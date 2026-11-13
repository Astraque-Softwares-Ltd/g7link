// SPDX-License-Identifier: GPL-3.0-or-later
// Generated from docs/design/tokens.json by `cargo xtask tokens`. Do not edit.
pragma Singleton
import QtQuick

QtObject {
    readonly property var data: ({
      "icons": {
        "set": "Material Symbols Rounded (Apache-2.0)",
        "size": 20,
        "stroke": "outlined, 400 weight; filled when selected"
      },
      "layout": {
        "desktop": {
          "contentPadding": 22,
          "gutter": 16,
          "minWindow": [
            880,
            600
          ],
          "railWidth": 80,
          "topBarHeight": 52
        },
        "phone": {
          "gutter": 10,
          "screenPadding": 16,
          "tabBarHeight": 64
        }
      },
      "motion": {
        "description": "Shared with the G7 Linux launcher; see docs/design/motion.md. Things leave faster than they arrive. Under reduced motion nothing moves or scales; page changes keep a quick crossfade.",
        "distance": {
          "pageSlide": 0.1,
          "popScale": 0.96,
          "rise": 8
        },
        "duration": {
          "large": {
            "enter": 260,
            "exit": 180,
            "use": "pages, tabs, sheets, expand and collapse"
          },
          "quick": {
            "enter": 120,
            "exit": 90,
            "use": "hover, press, color and selection changes, tooltips"
          },
          "small": {
            "enter": 200,
            "exit": 140,
            "use": "chips, banners, toasts, menus, popovers, list rows"
          }
        },
        "easing": {
          "enter": [
            0.05,
            0.7,
            0.1,
            1
          ],
          "exit": [
            0.3,
            0,
            0.8,
            0.15
          ],
          "move": [
            0.2,
            0,
            0,
            1
          ]
        },
        "fade": {
          "fast": 120,
          "normal": 200
        },
        "hoverLift": 2,
        "pressScale": 0.97,
        "spring": {
          "gentle": {
            "damping": 26,
            "stiffness": 220,
            "use": "page and panel transitions"
          },
          "snappy": {
            "damping": 34,
            "stiffness": 620,
            "use": "toggles, chips, buttons, hover lift"
          },
          "standard": {
            "damping": 30,
            "stiffness": 380,
            "use": "cards, sheets, expanding a notification into a conversation"
          }
        }
      },
      "spacing": {
        "scale": [
          0,
          2,
          4,
          6,
          8,
          10,
          12,
          16,
          20,
          24,
          32,
          40,
          48,
          64
        ],
        "unit": 4
      },
      "themes": {
        "quiet": {
          "description": "G7's Quiet Tool language (docs/design-language.md in g7linux). Charcoal and paper grounds, neutral selection, lavender only for focus, the primary button and the mark, Geist Mono everywhere, bare glyphs in their own tints, rows instead of cards. Colors are fixed.",
          "dynamicColor": false,
          "fonts": {
            "display": "Geist Mono",
            "label": "Geist Mono",
            "mono": "Geist Mono",
            "ui": "Geist Mono"
          },
          "label": {
            "case": "none",
            "size": 12,
            "tracking": 0,
            "weight": 500
          },
          "layout": {
            "columnWidth": 300,
            "footerHeight": 40,
            "headerHeight": 52,
            "rowHeight": 36,
            "rowPadding": 10,
            "rowRadius": 8,
            "sidebarWidth": 200
          },
          "shape": {
            "full": 8,
            "lg": 8,
            "md": 8,
            "sm": 6,
            "xl": 12,
            "xs": 5
          },
          "tints": {
            "dark": {
              "amber": "#EF9F27",
              "blue": "#7CB7EE",
              "coral": "#F0997B",
              "green": "#97C459",
              "lavender": "#B292D0",
              "pink": "#ED93B1",
              "red": "#E5736F",
              "teal": "#5DCAA5"
            },
            "light": {
              "amber": "#8F5B00",
              "blue": "#2F6DAE",
              "coral": "#B4532F",
              "green": "#4B7516",
              "lavender": "#7D5CA9",
              "pink": "#AE4470",
              "red": "#B3322D",
              "teal": "#1E7F64"
            }
          },
          "typography": {
            "body": {
              "lineHeight": 1.4,
              "size": 13,
              "tracking": 0,
              "weight": 400
            },
            "bodySmall": {
              "lineHeight": 1.4,
              "size": 12,
              "tracking": 0,
              "weight": 400
            },
            "caption": {
              "lineHeight": 1.4,
              "size": 11,
              "tracking": 0,
              "weight": 400
            },
            "code": {
              "lineHeight": 1.4,
              "size": 13,
              "tracking": 0,
              "weight": 500
            },
            "displayLarge": {
              "lineHeight": 1.3,
              "size": 22,
              "tracking": 0,
              "weight": 500
            },
            "displaySmall": {
              "lineHeight": 1.3,
              "size": 18,
              "tracking": 0,
              "weight": 500
            },
            "headline": {
              "lineHeight": 1.3,
              "size": 18,
              "tracking": 0,
              "weight": 500
            },
            "label": {
              "lineHeight": 1.4,
              "size": 12,
              "tracking": 0,
              "weight": 500
            },
            "title": {
              "lineHeight": 1.4,
              "size": 13,
              "tracking": 0,
              "weight": 500
            }
          },
          "variants": {
            "dark": {
              "accent": "#B292D0",
              "error": "#F0A39C",
              "onAccent": "#18181B",
              "onError": "#3B0B07",
              "onPrimary": "#18181B",
              "onPrimaryContainer": "#F2F2F3",
              "onSecondaryContainer": "#F2F2F3",
              "onSurface": "#F2F2F3",
              "onSurfaceVariant": "#A1A1A8",
              "outline": "#6F6F78",
              "outlineVariant": "#303035",
              "primary": "#F2F2F3",
              "primaryContainer": "#2B2B30",
              "secondaryContainer": "#2B2B30",
              "success": "#F2F2F3",
              "surface": "#18181B",
              "surfaceContainer": "#222226",
              "surfaceContainerHigh": "#2B2B30",
              "surfaceContainerHighest": "#303035",
              "surfaceContainerLow": "#18181B",
              "warning": "#E2C47A"
            },
            "light": {
              "accent": "#7D5CA9",
              "error": "#A4230F",
              "onAccent": "#FAFAFB",
              "onError": "#FFFFFF",
              "onPrimary": "#FAFAFB",
              "onPrimaryContainer": "#1C1C1F",
              "onSecondaryContainer": "#1C1C1F",
              "onSurface": "#1C1C1F",
              "onSurfaceVariant": "#5F5F67",
              "outline": "#7A7A83",
              "outlineVariant": "#DCDCE1",
              "primary": "#1C1C1F",
              "primaryContainer": "#E9E9ED",
              "secondaryContainer": "#E9E9ED",
              "success": "#1C1C1F",
              "surface": "#FAFAFB",
              "surfaceContainer": "#F1F1F3",
              "surfaceContainerHigh": "#E9E9ED",
              "surfaceContainerHighest": "#E4E4E9",
              "surfaceContainerLow": "#FAFAFB",
              "warning": "#6F5200"
            }
          }
        }
      }
    })
}
