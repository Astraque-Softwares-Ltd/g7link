// SPDX-License-Identifier: MPL-2.0
//! With no connection asking for sessions the host opens none of the
//! agents' files. Alone in its own test binary, so the process-wide count of
//! files the session reader opened is this test's alone.
#![cfg(feature = "fake-cli")]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use g7_agents::api::*;
use g7_agents::client::Client;
use g7_agents::host::sessions::tail;
use g7_agents::host::{self, Config};
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_subscriber_means_no_file_is_opened() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let run = root.join("run");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let claude = root.join("home/.claude");
    let project = claude.join("projects/-w");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(root.join("proc")).unwrap();
    std::fs::write(
        project.join("11111111-2222-4333-8444-555555555555.jsonl"),
        "{\"type\":\"user\",\"cwd\":\"/w\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
    )
    .unwrap();
    let config = Config {
        socket: run.join("host.sock"),
        runtime_dir: run,
        data_dir: root.join("data"),
        claude_bin: None,
        codex_bin: None,
        bridge_exe: PathBuf::from(env!("CARGO_BIN_EXE_g7-agents")),
        notify: false,
        env: vec![
            ("HOME".into(), root.join("home").into_os_string()),
            ("PATH".into(), "/nonexistent".into()),
        ],
        idle_exit: host::IDLE_EXIT,
        agent_idle: Duration::from_secs(600),
        approval_lifetime_ms: APPROVAL_LIFETIME_MS,
        claude_home: claude,
        codex_home: root.join("home/.codex"),
        proc_root: root.join("proc"),
        sessions: true,
        keep_awake_bus: false,
    };
    let socket = config.socket.clone();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(host::serve(config, async move {
        let _ = stopped.await;
    }));
    while !socket.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let hello = HelloParams::new(ClientKind::Test, &[]);
    let (client, _events) = Client::connect(&socket, hello).await.unwrap();
    client.subscribe(ThreadSelection::All, true).await.unwrap();
    client.threads_list(None, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(tail::opens(), 0, "a thread subscriber alone reads nothing");

    client
        .subscribe_with(SubscribeParams { threads: ThreadSelection::All, deltas: true, sessions: true })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(tail::opens() > 0, "with a sessions subscriber the scan runs");

    client.subscribe(ThreadSelection::All, true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = tail::opens();
    tokio::time::sleep(Duration::from_millis(6000)).await;
    assert_eq!(tail::opens(), after, "the loop stopped with its last subscriber");

    drop(client);
    let _ = stop.send(());
    task.await.unwrap().unwrap();
}
