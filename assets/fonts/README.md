# Fonts

The typefaces G7 Link ships with.
All are under the [SIL Open Font License 1.1](https://openfontlicense.org);
each license sits next to its font and is packaged with it.

| File | Family | Used for |
|---|---|---|
| `Figtree.ttf` | Figtree (variable, weights 300 to 900) | Text in both apps, and the virtual camera's "Start the webcam" card |
| `GeistMono-Variable.ttf` | Geist Mono (variable) | Codes, keys, commands and timers in both apps |

Figtree is taken unmodified from [google/fonts](https://github.com/google/fonts)
at commit `6e8069ff8ba3dab2a397fb30e7fbd243aba9b57a` (`ofl/figtree`).

The desktop app compiles both into its resources (`desktop/app/build.rs`), the
virtual camera compiles in Figtree (`desktop/vcam`), and the Android app
packages both as assets (`android/app/build.gradle.kts`).
