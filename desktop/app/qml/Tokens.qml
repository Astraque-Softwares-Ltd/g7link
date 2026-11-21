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
          "agents": {
            "dark": {
              "diff": {
                "addRow": {
                  "alpha": 0.24,
                  "color": "#97C459"
                },
                "addSign": "#B4D683",
                "addWord": {
                  "alpha": 0.44,
                  "color": "#97C459"
                },
                "delRow": {
                  "alpha": 0.13,
                  "color": "#E5736F"
                },
                "delSign": "#F0A39C",
                "delWord": {
                  "alpha": 0.35,
                  "color": "#E5736F"
                },
                "hunk": "#A1A1A8",
                "hunkGround": "#2B2B30",
                "lineNo": "#BDBDC4",
                "text": "#F2F2F3"
              },
              "ground": {
                "chip": "#2B2B30",
                "code": "#222226",
                "surface": "#18181B"
              },
              "prose": {
                "alertCaution": "#E5736F",
                "alertImportant": "#B292D0",
                "alertNote": "#7CB7EE",
                "alertTip": "#5DCAA5",
                "alertWarning": "#EF9F27",
                "doneBox": "#97C459",
                "headingRule": "#B292D0",
                "inlineCode": "#F0997B",
                "link": "#B292D0",
                "listMarker": "#A1A1A8",
                "tableHeader": "#A1A1A8",
                "thinking": "#A1A1A8"
              },
              "status": {
                "done": "#97C459",
                "failed": "#E5736F",
                "failedText": "#F0A39C",
                "failedWash": {
                  "alpha": 0.2,
                  "color": "#E5736F"
                },
                "mayNeedYou": "#EF9F27",
                "needsYou": "#EF9F27",
                "onNeedsYou": "#18181B",
                "paused": "#6F6F78",
                "working": "#7CB7EE"
              },
              "steps": {
                "addInk": "#B4D683",
                "delInk": "#F0A39C",
                "dim": "#BDBDC4",
                "lavenderInk": "#D6C7E7"
              },
              "syntax": {
                "attribute": "#EF9F27",
                "comment": "#A1A1A8",
                "constant": "#F0997B",
                "deleted": "#E5736F",
                "dir": "#A1A1A8",
                "error": "#F0A39C",
                "file": "#F2F2F3",
                "function": "#7CB7EE",
                "inserted": "#97C459",
                "keyword": "#ED93B1",
                "number": "#F0997B",
                "ok": "#97C459",
                "operator": "#BDBDC4",
                "plain": "#F2F2F3",
                "pos": "#A1A1A8",
                "punctuation": "#A1A1A8",
                "string": "#5DCAA5",
                "tag": "#ED93B1",
                "type": "#EF9F27",
                "variable": "#D6C7E7",
                "warn": "#EF9F27"
              },
              "tool": {
                "agent": "#B292D0",
                "command": "#5DCAA5",
                "edit": "#F0997B",
                "mcp": "#A1A1A8",
                "other": "#A1A1A8",
                "plan": "#A1A1A8",
                "read": "#7CB7EE",
                "search": "#ED93B1",
                "tasks": "#A1A1A8",
                "web": "#7CB7EE"
              },
              "toolStatus": {
                "asking": "#EF9F27",
                "denied": "#A1A1A8",
                "failed": "#E5736F",
                "ok": "#97C459",
                "running": "#7CB7EE",
                "stopped": "#A1A1A8"
              }
            },
            "glyphOnly": [
              "status.paused",
              "prose.headingRule"
            ],
            "light": {
              "diff": {
                "addRow": {
                  "alpha": 0.1,
                  "color": "#4B7516"
                },
                "addSign": "#3B5C10",
                "addWord": {
                  "alpha": 0.32,
                  "color": "#4B7516"
                },
                "delRow": {
                  "alpha": 0.26,
                  "color": "#B3322D"
                },
                "delSign": "#8E1D0C",
                "delWord": {
                  "alpha": 0.46,
                  "color": "#B3322D"
                },
                "hunk": "#5F5F67",
                "hunkGround": "#E9E9ED",
                "lineNo": "#45454C",
                "text": "#1C1C1F"
              },
              "ground": {
                "chip": "#E9E9ED",
                "code": "#F1F1F3",
                "surface": "#FAFAFB"
              },
              "prose": {
                "alertCaution": "#B3322D",
                "alertImportant": "#7858A2",
                "alertNote": "#2E6AA9",
                "alertTip": "#1C755C",
                "alertWarning": "#8F5B00",
                "doneBox": "#4A7316",
                "headingRule": "#7D5CA9",
                "inlineCode": "#A74D2C",
                "link": "#7D5CA9",
                "listMarker": "#5F5F67",
                "tableHeader": "#5F5F67",
                "thinking": "#5F5F67"
              },
              "status": {
                "done": "#4A7316",
                "failed": "#8E1D0C",
                "failedText": "#8E1D0C",
                "failedWash": {
                  "alpha": 0.2,
                  "color": "#B3322D"
                },
                "mayNeedYou": "#8F5B00",
                "needsYou": "#8F5B00",
                "onNeedsYou": "#FAFAFB",
                "paused": "#7A7A83",
                "working": "#2E6AA9"
              },
              "steps": {
                "addInk": "#3B5C10",
                "delInk": "#8E1D0C",
                "dim": "#45454C",
                "lavenderInk": "#4F3A73"
              },
              "syntax": {
                "attribute": "#8F5B00",
                "comment": "#5F5F67",
                "constant": "#A74D2C",
                "deleted": "#8E1D0C",
                "dir": "#5F5F67",
                "error": "#8E1D0C",
                "file": "#1C1C1F",
                "function": "#2E6AA9",
                "inserted": "#4A7316",
                "keyword": "#AB436E",
                "number": "#A74D2C",
                "ok": "#4A7316",
                "operator": "#45454C",
                "plain": "#1C1C1F",
                "pos": "#5F5F67",
                "punctuation": "#5F5F67",
                "string": "#1C755C",
                "tag": "#AB436E",
                "type": "#8F5B00",
                "variable": "#4F3A73",
                "warn": "#8F5B00"
              },
              "tool": {
                "agent": "#7D5CA9",
                "command": "#1E7F64",
                "edit": "#B4532F",
                "mcp": "#5F5F67",
                "other": "#5F5F67",
                "plan": "#5F5F67",
                "read": "#2F6DAE",
                "search": "#AE4470",
                "tasks": "#5F5F67",
                "web": "#2F6DAE"
              },
              "toolStatus": {
                "asking": "#8F5B00",
                "denied": "#5F5F67",
                "failed": "#8E1D0C",
                "ok": "#4A7316",
                "running": "#2E6AA9",
                "stopped": "#5F5F67"
              }
            },
            "mustDiffer": [
              {
                "a": "diff.addRow",
                "also": "+ and −",
                "b": "diff.delRow"
              },
              {
                "a": "diff.addWord",
                "also": "strike on removed",
                "b": "diff.delWord"
              },
              {
                "a": "toolStatus.ok",
                "also": "check and x",
                "b": "toolStatus.failed"
              },
              {
                "a": "syntax.inserted",
                "also": "the line's own sign",
                "b": "syntax.deleted"
              },
              {
                "a": "status.needsYou",
                "also": "fill, glyph",
                "b": "status.failed"
              },
              {
                "a": "status.needsYou",
                "also": "spinner",
                "b": "ground.surface"
              },
              {
                "a": "prose.link",
                "also": "underline",
                "b": "syntax.plain"
              },
              {
                "a": "prose.inlineCode",
                "also": "mono",
                "b": "syntax.plain"
              },
              {
                "a": "syntax.comment",
                "also": "italic",
                "b": "syntax.plain"
              },
              {
                "a": "syntax.keyword",
                "also": "weight",
                "b": "syntax.plain"
              }
            ],
            "styles": {
              "comment": "italic",
              "keyword": "semibold",
              "tag": "semibold"
            }
          },
          "description": "G7's Quiet Tool language (docs/design-language.md in g7linux). Charcoal and paper grounds, neutral selection, lavender only for focus, the primary button and the mark, Figtree for text and Geist Mono for codes and keys, bare glyphs in their own tints, rows instead of cards. Colors are fixed.",
          "dynamicColor": false,
          "fonts": {
            "display": "Figtree",
            "label": "Figtree",
            "mono": "Geist Mono",
            "ui": "Figtree"
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
          "tintsText": {
            "light": {
              "amber": "#8F5B00",
              "blue": "#2E6AA9",
              "coral": "#A74D2C",
              "green": "#4A7316",
              "lavender": "#7858A2",
              "pink": "#AB436E",
              "red": "#B3322D",
              "teal": "#1C755C"
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
