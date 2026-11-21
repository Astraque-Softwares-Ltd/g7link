// SPDX-License-Identifier: MPL-2.0
//! Bringing sessions in (§U4): the `.zst` reader's limits on frames made by
//! hand, a scan past its bound, and the account ids in a rollout's first
//! line never reaching the database. Every file is written by the test.
#![cfg(feature = "fake-cli")]

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use g7_agents::api::*;
use g7_agents::host::db::{Db, FoundQuery};
use g7_agents::host::sessions::FoundStore;
use g7_agents::host::sessions::import::{self, Keep, Segments, testing::DbStore};
use g7_agents::host::sessions::paths::Roots;
use g7_agents::host::sessions::scan::{self, ScanJob};
use g7_agents::host::sessions::zst::{self, ZstReader};
use serde_json::json;

const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// A frame header with no content size: `window_log` gives the window.
fn header(window_log: u8) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.push(0x00);
    out.push((window_log - 10) << 3);
    out
}

/// A block header: last, type (0 raw, 1 RLE), size.
fn block(last: bool, kind: u32, size: u32) -> [u8; 3] {
    let v = (size << 3) | (kind << 1) | u32::from(last);
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

fn drain(mut r: impl Read) -> io::Result<u64> {
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0;
    loop {
        match r.read(&mut buf)? {
            0 => return Ok(total),
            n => total += n as u64,
        }
    }
}

#[test]
fn a_frame_asking_for_a_2_gib_window_is_refused_before_anything_is_allocated() {
    let mut frame = header(31);
    frame.extend_from_slice(&block(true, 0, 1));
    frame.push(b'x');
    let e = ZstReader::new(frame.as_slice(), zst::MAX_DECODED).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    // The same frame with a 2 MiB window, as Codex writes them, reads.
    let mut ok = header(21);
    ok.extend_from_slice(&block(true, 0, 1));
    ok.push(b'x');
    let mut out = Vec::new();
    ZstReader::new(ok.as_slice(), zst::MAX_DECODED).unwrap().read_to_end(&mut out).unwrap();
    assert_eq!(out, b"x");
}

#[test]
fn a_bomb_stops_at_the_limit() {
    // 128 KiB blocks of one repeated byte: four bytes each, the most zstd
    // can expand; 16 KiB of them make 537 MiB.
    let mut bomb = header(17);
    let blocks = 4097;
    for n in 0..blocks {
        bomb.extend_from_slice(&block(n + 1 == blocks, 1, 128 * 1024));
        bomb.push(b'\n');
    }
    assert!(bomb.len() < 17 * 1024);
    let reader = ZstReader::new(bomb.as_slice(), zst::MAX_DECODED).unwrap();
    let e = drain(reader).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::FileTooLarge);
}

#[test]
fn a_cut_frame_is_an_error() {
    let body: Vec<u8> = (0..5000).flat_map(|n| format!("{{\"ordinal\":{n}}}\n").into_bytes()).collect();
    let packed =
        ruzstd::encoding::compress_to_vec(body.as_slice(), ruzstd::encoding::CompressionLevel::Fastest);
    let mut out = Vec::new();
    ZstReader::new(packed.as_slice(), zst::MAX_DECODED).unwrap().read_to_end(&mut out).unwrap();
    assert_eq!(out, body);
    let cut = &packed[..packed.len() - 7];
    assert!(ZstReader::new(cut, zst::MAX_DECODED).and_then(drain).is_err());
}

fn roots(dir: &Path) -> Roots {
    Roots {
        claude_home: dir.join("claude"),
        codex_home: dir.join("codex"),
        proc_root: dir.join("proc"),
        claude_app_dir: None,
    }
}

fn job(roots: &Roots) -> ScanJob {
    ScanJob {
        roots: roots.clone(),
        folders: Vec::new(),
        titles: None,
        refused: HashMap::new(),
        stop: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn twenty_thousand_and_one_transcripts_make_a_partial_scan() {
    let dir = tempfile::tempdir().unwrap();
    let roots = roots(dir.path());
    let line = |n: u32| {
        format!(
            "{}\n",
            json!({"type": "user", "uuid": "u", "cwd": "/w", "message": {"role": "user", "content": format!("p{n}")}})
        )
    };
    let mut n = 0u32;
    'folders: for folder in 0..6 {
        let dir = roots.claude_home.join("projects").join(format!("-w-{folder}"));
        std::fs::create_dir_all(&dir).unwrap();
        for _ in 0..4000 {
            if n == 20_001 {
                break 'folders;
            }
            std::fs::write(dir.join(format!("{n:08x}-0000-4000-8000-000000000000.jsonl")), line(n)).unwrap();
            n += 1;
        }
    }
    let store = DbStore::new(Db::open(&dir.path().join("db")).unwrap());
    let report = scan::run(job(&roots), &store, &mut |_| {}).unwrap();
    assert!(report.partial);
    assert_eq!(report.seen, 20_000);
    let page = store.found(&FoundQuery { limit: 1, ..FoundQuery::default() }).unwrap();
    assert_eq!(page.total, 20_000);
}

#[test]
fn account_ids_in_a_rollout_never_reach_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let roots = roots(dir.path());
    let id = "11111111-2222-4333-8444-555555555555";
    let day = roots.codex_home.join("sessions/2026/11/19");
    std::fs::create_dir_all(&day).unwrap();
    let lines = [
        json!({"timestamp": "2026-11-19T09:00:00Z", "ordinal": 0, "type": "session_meta",
               "payload": {"id": id, "cwd": "/w", "originator": "codex-tui",
                           "creator_account_id": "acct-SECRET-0042", "creator_user_id": "user-SECRET-0042"}}),
        json!({"timestamp": "2026-11-19T09:00:01Z", "ordinal": 1, "type": "event_msg",
               "payload": {"type": "item_completed", "item": {"type": "UserMessage", "id": "u1",
                           "content": [{"type": "text", "text": "Explain the build."}]}}}),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let packed =
        ruzstd::encoding::compress_to_vec(body.as_bytes(), ruzstd::encoding::CompressionLevel::Fastest);
    std::fs::write(day.join(format!("rollout-2026-11-19T09-00-00-{id}.jsonl.zst")), packed).unwrap();
    let data = dir.path().join("db");
    let store = Arc::new(DbStore::new(Db::open(&data).unwrap()));
    scan::run(job(&roots), &*store, &mut |_| {}).unwrap();
    let found_id = format!("sx{}", id.replace('-', ""));
    let row = store.found_row(&found_id).unwrap().unwrap();
    let mut segments = Segments::new(store.clone());
    let reading = import::read_session(&row, &mut segments, Keep::import(), &AtomicBool::new(false)).unwrap();
    let thread = import::brought_in(&row, reading, &[], Origin::launcher(), false);
    assert_eq!(thread.folder, None, "outside every agent folder");
    let thread_id = store.bring_in(thread).unwrap();
    let loaded = store.db().load_thread(&thread_id).unwrap().unwrap();
    assert_eq!(loaded.info.readonly, Some(Readonly::OutsideFolders));
    assert_eq!(loaded.cols.cwd.as_deref(), Some("/w"));
    assert!(!loaded.info.unread);
    drop(segments);
    drop(store);
    let mut bytes = Vec::new();
    for entry in std::fs::read_dir(&data).unwrap().flatten() {
        let path: PathBuf = entry.path();
        if path.is_file() {
            bytes.extend(std::fs::read(&path).unwrap());
        }
    }
    assert!(!bytes.is_empty());
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("Explain the build."), "the message is stored");
    assert!(!text.contains("SECRET"), "the account ids are not");
}
