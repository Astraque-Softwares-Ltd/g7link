# Fonts

The typefaces G7 Link ships with.
All are under the [SIL Open Font License 1.1](https://openfontlicense.org);
each license sits next to its font and is packaged with it.

| File | Family | Used for |
|---|---|---|
| `GeistMono-Variable.ttf` | Geist Mono (variable) | All text in both apps, the same file G7 ships |
| `Figtree.ttf` | Figtree (variable, weights 300 to 900) | The virtual camera's "Start the webcam" card |

Figtree is taken unmodified from [google/fonts](https://github.com/google/fonts)
at commit `6e8069ff8ba3dab2a397fb30e7fbd243aba9b57a` (`ofl/figtree`).

The desktop app compiles Geist Mono into its resources (`desktop/app/build.rs`)
and the virtual camera Figtree (`desktop/vcam`); the Android app packages Geist
Mono as an asset (`android/app/build.gradle.kts`).
