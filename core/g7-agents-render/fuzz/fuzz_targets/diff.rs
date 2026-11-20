// SPDX-License-Identifier: MPL-2.0
//! Diff parsing on arbitrary text.

#![no_main]

use g7_agents_render::bounds::{MAX_DIFF_LINE_UNITS, MAX_DIFF_ROWS};
use g7_agents_render::diff;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let doc = diff(&text);
    let rows: usize = doc.files.iter().map(|f| f.rows.len()).sum();
    assert!(rows + doc.files.len() <= MAX_DIFF_ROWS);
    for file in &doc.files {
        for row in &file.rows {
            let units = row.text.encode_utf16().count() as u32;
            assert!(units as usize <= MAX_DIFF_LINE_UNITS);
            for &(start, len) in &row.marks {
                assert!(len > 0 && start + len <= units);
            }
        }
    }
});
