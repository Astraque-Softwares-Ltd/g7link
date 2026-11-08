# G7 Link PowerToys Command Palette / PowerToys Run Extension (`g7link-ptrun`)

`g7link-ptrun` is a standalone, zero-dependency Rust helper that connects Windows launcher tools (PowerToys Command Palette, PowerToys Run, Flow Launcher, Raycast for Windows) to G7 Link's [Local Authenticated API](../../docs/api.md).

## Building

```powershell
cargo build --release --manifest-path tools/powertoys-extension/Cargo.toml
```

## Prerequisites

1. Open **G7 Link → Settings → Local automation API** and turn on **Enable local automation API**.
2. G7 Link writes `%LOCALAPPDATA%\G7Link\api-token.json` containing the loopback port (`53320`) and a 256-bit Bearer token.

## Usage

### Querying Palette Items

```powershell
g7link-ptrun query ""
g7link-ptrun query "ring"
g7link-ptrun query "https://g7.astraque.io"
```

Outputs a JSON array of palette commands (`send-clipboard`, `ring`, `camera` `photo`/`scan`, `send-url`, `send-text`, plus recent timeline items).

### Invoking an Action

```powershell
g7link-ptrun invoke ring
g7link-ptrun invoke send-clipboard
g7link-ptrun invoke camera photo
g7link-ptrun invoke camera scan
g7link-ptrun invoke send-url "https://g7.astraque.io"
g7link-ptrun invoke send-text "Copied from PowerToys"
g7link-ptrun invoke send-file "C:\Users\Alice\Desktop\notes.pdf"
```
