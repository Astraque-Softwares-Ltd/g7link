// SPDX-License-Identifier: GPL-3.0-or-later
//! QR codes: as one SVG path, so QML draws them crisp at any size and scale,
//! or as text to print in a terminal.

use qrcode::{Color, EcLevel, QrCode};

/// A QR code ready to draw: `path` covers the dark modules of a
/// `size`×`size` grid (one unit per module, no quiet zone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrPath {
    pub size: usize,
    pub path: String,
}

/// Encodes `text` with medium error correction (pairing links are short and
/// get scanned off screens, where M is the usual choice).
pub fn encode(text: &str) -> Result<QrPath, qrcode::types::QrError> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)?;
    let size = code.width();
    let colors = code.to_colors();
    let mut path = String::with_capacity(size * size);
    for (y, row) in colors.chunks(size).enumerate() {
        let mut x = 0;
        while x < size {
            if row[x] != Color::Dark {
                x += 1;
                continue;
            }
            // Merge horizontal runs into one rectangle each.
            let start = x;
            while x < size && row[x] == Color::Dark {
                x += 1;
            }
            let run = x - start;
            path.push_str(&format!("M{start} {y}h{run}v1h-{run}z"));
        }
    }
    Ok(QrPath { size, path })
}

/// The modules of blank border scanners need around a code.
const QUIET_ZONE: usize = 4;

/// `text` as a QR code to print in a terminal: two rows of modules per line,
/// drawn with half blocks, inside its quiet zone. With `ansi`, each line sets
/// black on white, so the code scans whatever the terminal's colors.
pub fn terminal(text: &str, ansi: bool) -> Result<String, qrcode::types::QrError> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L)?;
    let size = code.width();
    let colors = code.to_colors();
    let full = size + 2 * QUIET_ZONE;
    let code_area = QUIET_ZONE..QUIET_ZONE + size;
    let dark = |x: usize, y: usize| {
        code_area.contains(&x)
            && code_area.contains(&y)
            && colors[(y - QUIET_ZONE) * size + x - QUIET_ZONE] == Color::Dark
    };
    let mut out = String::with_capacity(full.div_ceil(2) * (full * 3 + 16));
    for y in (0..full).step_by(2) {
        if ansi {
            out.push_str("\x1b[30;107m");
        }
        for x in 0..full {
            out.push(match (dark(x, y), dark(x, y + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        if ansi {
            out.push_str("\x1b[0m");
        }
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turns the path back into a grid to check it matches the code.
    fn rasterize(qr: &QrPath) -> Vec<bool> {
        let mut grid = vec![false; qr.size * qr.size];
        for rect in qr.path.split('z').filter(|r| !r.is_empty()) {
            let nums: Vec<usize> = rect
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect();
            let [x, y, run, ..] = nums[..] else { panic!("bad rect {rect}") };
            for i in x..x + run {
                grid[y * qr.size + i] = true;
            }
        }
        grid
    }

    #[test]
    fn terminal_code_has_its_quiet_zone_and_finder_pattern() {
        let drawn = terminal("https://example.com/app.apk", false).unwrap();
        let lines: Vec<&str> = drawn.lines().collect();
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|line| line.chars().count() == width));
        assert_eq!(lines.len(), width.div_ceil(2));
        assert!(lines[0].trim().is_empty() && lines[1].trim().is_empty());
        assert!(lines[lines.len() - 1].trim().is_empty());
        // The top edge of the top-left finder: seven dark modules after the quiet zone.
        let edge: String = lines[2].chars().skip(QUIET_ZONE).take(7).collect();
        assert_eq!(edge, "█▀▀▀▀▀█");
        assert!(lines[2].starts_with("    "));
    }

    #[test]
    fn terminal_code_sets_its_colors_when_asked() {
        let drawn = terminal("https://example.com", true).unwrap();
        for line in drawn.lines() {
            assert!(line.starts_with("\x1b[30;107m") && line.ends_with("\x1b[0m"), "{line:?}");
        }
        assert!(!terminal("https://example.com", false).unwrap().contains('\x1b'));
    }

    #[test]
    fn path_matches_the_code() {
        let text = "g7link://pair?v=0&id=ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqx&s=abcdefghijklmnopqrstuvwxyz";
        let qr = encode(text).unwrap();
        let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).unwrap();
        let expected: Vec<bool> = code.to_colors().into_iter().map(|c| c == Color::Dark).collect();
        assert_eq!(qr.size, code.width());
        assert_eq!(rasterize(&qr), expected);
    }

    #[test]
    fn longer_text_needs_a_bigger_code() {
        assert!(encode(&"x".repeat(200)).unwrap().size > encode("x").unwrap().size);
    }
}
