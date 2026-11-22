// SPDX-License-Identifier: MPL-2.0
//! Source control on a PC's agent threads, from a phone
//! (`docs/protocol/agents.md` §8): two real nodes over localhost (three for
//! another phone's review), a PC whose platform plays the agent host, and
//! a phone that reads, commits, pushes and opens pull requests.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use g7link_core::{
    AgentBranch, AgentBranches, AgentCheck, AgentGhState, AgentGit, AgentGitChanged, AgentGitFile,
    AgentGitPatchPage, AgentGitRemote, AgentGitReviewed, AgentGitReviewedFile, AgentGitWorktree, AgentJob,
    AgentJobError, AgentPrBrief, AgentPrComment, AgentPrCreateRequest, AgentPrDetail, AgentPrDetailRequest,
    AgentPrFile, AgentPrInfo, AgentPrListRequest, AgentPrRow, AgentPrRows, AgentPrView, AgentReply,
    AgentRequest, AgentThread, AgentWrote, DeviceId, DeviceInfo, DeviceKind, Error, LinkState, Node,
    NodeConfig, NodeEvent, PairingEvent, PlainKeyProtector, Platform, PowerLevel, agent, git, pr,
};
use g7link_protocol::messages::types;
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;

const WAIT: Duration = Duration::from_secs(20);
const FOLDER: &str = "f0123456789ab";
const THREAD: &str = "0123456789abcdef0123456789abcdef";
/// The host's id of every review it pins.
const HOST_REVIEW: &str = "00000000000000aa";
/// A job's id on the host.
const JOB: &str = "0123456789abcdef";
/// Files whose patch comes in two pages.
const SMALL: &str = "src/sync.rs";
const OTHER: &str = "src/retry.rs";
/// A file whose patch never ends: the PC stops at 1 MiB, so it's never read whole.
const ENDLESS: &str = "big/endless.txt";
/// A file too large for a phone.
const HUGE: &str = "assets/huge.bin";
/// An untracked folder with three files.
const DOCS: &str = "docs/";
const PAGE: usize = 196_608;
/// A commit message the host refuses before it runs (`thread_busy`).
const BUSY_MESSAGE: &str = "Busy now";

/// A PC's agent host, as far as these tests need one.
#[derive(Debug, Default)]
struct HostPlatform {
    requests: Mutex<Vec<(DeviceId, AgentRequest)>>,
}

fn thread(id: &str) -> AgentThread {
    AgentThread {
        id: id.into(),
        agent: "claude".into(),
        folder: "g7linux".into(),
        title: "Fix the sync retry".into(),
        state: "done".into(),
        mode: "supervised".into(),
        updated: 1_795_000_000_000,
        branch: Some("feat/sync-retry".into()),
        changed: Some(4),
        ..Default::default()
    }
}

/// A status as a careless host might give it: a URL, absolute paths and a
/// worktree's path where a phone may see none of them, and 700 files.
fn status() -> AgentGit {
    let mut files = vec![
        AgentGitFile { path: SMALL.into(), status: Some("modified".into()), ..Default::default() },
        AgentGitFile {
            path: DOCS.into(),
            status: Some("untracked".into()),
            folder: true,
            ..Default::default()
        },
        AgentGitFile { path: "/home/me/.ssh/id_ed25519".into(), ..Default::default() },
        AgentGitFile { path: "../../etc/passwd".into(), ..Default::default() },
    ];
    files.extend((0..696).map(|i| AgentGitFile { path: format!("gen/f{i}.rs"), ..Default::default() }));
    AgentGit {
        repo: git::repo::OK.into(),
        branch: Some("feat/sync-retry".into()),
        upstream: Some("origin/feat/sync-retry".into()),
        ahead: Some(2),
        behind: Some(1),
        remote: Some("origin".into()),
        remotes: vec![
            AgentGitRemote { name: "origin".into(), label: Some("github.com/acme/app".into()), local: false },
            AgentGitRemote {
                name: "mirror".into(),
                label: Some("https://me:hunter2@git.example.com/app.git".into()),
                local: false,
            },
            AgentGitRemote { name: "backup".into(), label: Some("/srv/git/app.git".into()), local: true },
        ],
        default_branch: Some("main".into()),
        files,
        total: 700,
        worktree: Some(AgentGitWorktree {
            label: "/home/me/.local/share/g7-agents/worktrees/f0123456789ab/sync-1a2b3c".into(),
            branch: "g7/sync".into(),
        }),
        fingerprint: "0123456789abcdef".into(),
        checked_ms: 1_795_000_000_000,
        ..Default::default()
    }
}

fn job(kind: &str, phase: &str, lines: usize) -> AgentJob {
    AgentJob {
        id: JOB.into(),
        thread: THREAD.into(),
        kind: kind.into(),
        phase: phase.into(),
        started_ms: 1_795_000_000_000,
        lines: (0..lines).map(|i| format!("hook line {i}")).collect(),
        ..Default::default()
    }
}

fn gh() -> AgentGhState {
    AgentGhState {
        gh: pr::gh::OK.into(),
        version: Some("2.102.0".into()),
        hosts: vec!["github.com".into()],
        checked_ms: 1,
    }
}

fn pr_info(checks: usize) -> AgentPrInfo {
    AgentPrInfo {
        number: 212,
        title: "Fix the sync retry".into(),
        state: pr::state::OPEN.into(),
        url: "https://github.com/acme/app/pull/212".into(),
        base: "main".into(),
        head: "feat/sync-retry".into(),
        checks: (0..checks)
            .map(|i| AgentCheck { name: format!("ci / job {i}"), state: "pass".into(), url: None })
            .collect(),
        pass: u32::try_from(checks).unwrap(),
        ..Default::default()
    }
}

/// A pull request's detail as GitHub might give it through a careless
/// host: control characters, a path out of the repository, 100 comments.
fn pr_detail() -> AgentPrDetail {
    let mut files = vec![
        AgentPrFile { path: SMALL.into(), change: "modified".into(), adds: 2, dels: 1 },
        AgentPrFile { path: "../../etc/passwd".into(), change: "modified".into(), ..Default::default() },
    ];
    files.extend((0..350).map(|i| AgentPrFile {
        path: format!("gen/f{i}.rs"),
        change: "added".into(),
        adds: 1,
        dels: 0,
    }));
    AgentPrDetail {
        pr: Some(pr_info(3)),
        gh: gh(),
        repo: "acme/app".into(),
        author: "octo\u{202E}cat".into(),
        body: format!("Fixes the retry.\u{1b}[2J\n{}", "b".repeat(40_000)),
        adds: 352,
        dels: 1,
        changed: 352,
        files,
        comments: (0..100u64)
            .map(|i| AgentPrComment {
                author: format!("rev{i}"),
                at_ms: 1_795_000_000_000 + i,
                body: "c".repeat(4_000),
                review: (i == 99).then(|| "approved".to_owned()),
                url: Some(format!("https://github.com/acme/app/pull/212#c{i}")),
                ..Default::default()
            })
            .collect(),
        comments_older: 3,
        thread: Some(THREAD.into()),
        read_ms: 1_795_000_000_000,
        ..Default::default()
    }
}

impl HostPlatform {
    fn requests(&self) -> Vec<(DeviceId, AgentRequest)> {
        self.requests.lock().unwrap().clone()
    }

    /// The requests that reached the host, without the phone each came from.
    fn asked(&self) -> Vec<AgentRequest> {
        self.requests().into_iter().map(|(_, r)| r).collect()
    }

    fn commits(&self) -> Vec<AgentRequest> {
        self.asked().into_iter().filter(|r| matches!(r, AgentRequest::GitCommit { .. })).collect()
    }
}

impl Platform for HostPlatform {
    fn agent_request(
        &self,
        from: &DeviceId,
        request: AgentRequest,
    ) -> Result<AgentReply, g7link_core::AgentFailure> {
        self.requests.lock().unwrap().push((*from, request.clone()));
        let by = Some(*from);
        Ok(match request {
            AgentRequest::GitStatus { .. } => AgentReply::Git(status()),
            AgentRequest::GitPatch { path, offset, .. } => {
                let at = offset.unwrap_or(0);
                AgentReply::GitPatch(match path.as_str() {
                    ENDLESS => AgentGitPatchPage {
                        patch: format!("+{}\n", "x".repeat(PAGE - 2)),
                        next: Some(at + PAGE as u64),
                        ..Default::default()
                    },
                    DOCS => AgentGitPatchPage {
                        names: vec!["docs/a.md".into(), "docs/b.md".into(), "/etc/shadow".into()],
                        ..Default::default()
                    },
                    _ if at == 0 => AgentGitPatchPage {
                        patch: "@@ -1 +1 @@\n".into(),
                        next: Some(12),
                        ..Default::default()
                    },
                    _ => AgentGitPatchPage { patch: "-a\n+b\n".into(), ..Default::default() },
                })
            }
            AgentRequest::GitReview { paths, .. } => AgentReply::Review(AgentGitReviewed {
                review: HOST_REVIEW.into(),
                files: paths
                    .iter()
                    .map(|p| AgentGitReviewedFile {
                        path: p.clone(),
                        bytes: if p == HUGE { 2 << 20 } else { 100 },
                        folder: p.ends_with('/'),
                        entries: if p.ends_with('/') { 3 } else { 0 },
                    })
                    .collect(),
                pc_only: false,
            }),
            AgentRequest::GitCommit { message, .. } if message == BUSY_MESSAGE => {
                AgentReply::Refused(AgentJobError {
                    reason: "thread_busy".into(),
                    message: "Wait until Claude finishes.".into(),
                    names: Vec::new(),
                })
            }
            AgentRequest::GitCommit { .. } => {
                AgentReply::GitJob { job: job(git::kind::COMMIT, git::phase::RUNNING, 3), by }
            }
            AgentRequest::GitPush { .. } => {
                AgentReply::GitJob { job: job(git::kind::PUSH, git::phase::RUNNING, 0), by }
            }
            AgentRequest::GitFetch { .. } => {
                AgentReply::GitJob { job: job(git::kind::FETCH, git::phase::DONE, 1), by: None }
            }
            AgentRequest::GitSwitch { .. } => {
                AgentReply::GitJob { job: job(git::kind::SWITCH, git::phase::RUNNING, 0), by }
            }
            AgentRequest::GitJob { .. } | AgentRequest::GitStop { .. } => {
                AgentReply::GitJob { job: job(git::kind::COMMIT, git::phase::STOPPED, 400), by }
            }
            AgentRequest::GitBranches { .. } => AgentReply::Branches(AgentBranches {
                branches: (0..250)
                    .map(|i| AgentBranch {
                        name: format!("feat/b{i}"),
                        worktree: (i == 1).then(|| "/home/me/wt".to_owned()),
                        ..Default::default()
                    })
                    .collect(),
                more: false,
                current: Some("feat/b0".into()),
            }),
            AgentRequest::PrView { .. } => {
                AgentReply::Pr(AgentPrView { pr: Some(pr_info(150)), gh: gh(), github: None, why: None })
            }
            AgentRequest::PrCreate { .. } => {
                AgentReply::GitJob { job: job(git::kind::PR_CREATE, git::phase::RUNNING, 0), by }
            }
            AgentRequest::PrList { .. } => AgentReply::PrRows(AgentPrRows {
                prs: (1..=70)
                    .map(|n| AgentPrRow {
                        number: n,
                        title: format!("PR {n}"),
                        repo: "acme/app".into(),
                        state: pr::state::OPEN.into(),
                        url: format!("https://github.com/acme/app/pull/{n}"),
                        ..Default::default()
                    })
                    .collect(),
                more: false,
                gh: gh(),
            }),
            AgentRequest::PrDetail { .. } => AgentReply::PrDetail(pr_detail()),
            AgentRequest::PrDiff { path, offset, .. } => {
                let at = offset.unwrap_or(0);
                AgentReply::GitPatch(match path.as_str() {
                    ENDLESS => AgentGitPatchPage {
                        patch: format!("+{}\n", "x".repeat(PAGE - 2)),
                        next: Some(at + PAGE as u64),
                        ..Default::default()
                    },
                    _ if at == 0 => AgentGitPatchPage {
                        patch: "diff --git a/src/sync.rs b/src/sync.rs\n".into(),
                        next: Some(40),
                        // A pull request's diff has no folder names: never passed on.
                        names: vec!["docs/a.md".into()],
                        ..Default::default()
                    },
                    _ => AgentGitPatchPage { patch: "-a\n+b\n".into(), ..Default::default() },
                })
            }
            AgentRequest::AskReview { thread: id, .. } => AgentReply::Thread(thread(&id)),
            AgentRequest::Ask { thread: id, purpose } => {
                let mut t = thread(&id);
                t.wrote = Some(AgentWrote { purpose, turn: 3, text: "Fix the retry".into(), at_ms: 1 });
                AgentReply::Thread(t)
            }
            _ => return Err(g7link_core::AgentFailure::Unsupported),
        })
    }
}

struct Device {
    node: Node,
    events: Receiver<NodeEvent>,
    host: Arc<HostPlatform>,
    _dir: TempDir,
    name: &'static str,
}

fn info(name: &str, kind: DeviceKind) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        kind,
        os: if kind == DeviceKind::Phone { "android" } else { "linux" }.into(),
        os_ver: "test".into(),
        model: None,
        accent: None,
        screen: None,
        install_id: None,
    }
}

async fn start(name: &'static str, kind: DeviceKind, offers: &[&str]) -> Device {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let mut config = NodeConfig::new(dir.path(), info(name, kind), "0.0.1-test");
    config.lan_discovery = false;
    config.away_mode = false;
    config.key_protector = Some(Arc::new(PlainKeyProtector));
    let host = Arc::new(HostPlatform::default());
    let node = Node::start(config, host.clone()).await.expect("node starts");
    node.update_power(PowerLevel::NotApplicable, offers.iter().map(|c| (*c).to_owned()).collect()).await;
    let events = node.events();
    Device { node, events, host, _dir: dir, name }
}

async fn wait_for<T>(dev: &mut Device, what: &str, mut pick: impl FnMut(&NodeEvent) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, dev.events.recv()).await {
            Ok(Ok(event)) => {
                if let Some(found) = pick(&event) {
                    return found;
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("{}: event stream ended while waiting for {what}: {e}", dev.name),
            Err(_) => panic!("{}: timed out waiting for {what}", dev.name),
        }
    }
}

fn catalog() -> g7link_core::AgentCatalog {
    g7link_core::AgentCatalog {
        agents: vec![g7link_core::AgentCatalogAgent {
            id: "claude".into(),
            name: "Claude".into(),
            installed: true,
            signed_in: true,
            login_hint: "claude auth login".into(),
            ..Default::default()
        }],
        folders: vec![g7link_core::AgentCatalogFolder {
            id: FOLDER.into(),
            label: "g7linux".into(),
            checkpoints: true,
            follow_remote: true,
            ..Default::default()
        }],
        modes: vec!["supervised".into()],
        // The platform's word is never taken: the core says it per phone.
        push: true,
    }
}

/// Waits for a catalog from `pc` on `phone` whose `push` is `push`.
async fn wait_catalog(phone: &mut Device, pc: DeviceId, push: bool) {
    wait_for(phone, if push { "catalog with push" } else { "catalog without push" }, |e| match e {
        NodeEvent::AgentCatalog { device, catalog: Some(c), .. } if *device == pc && c.push == push => {
            Some(())
        }
        _ => None,
    })
    .await
}

/// A phone that follows `pc`'s agents, paired with it, with the `agents` toggle on.
async fn add_phone(pc: &mut Device, name: &'static str) -> Device {
    let mut phone = start(name, DeviceKind::Phone, &[agent::ITEMS2, agent::CONTROL]).await;
    let uri = pc.node.pairing_start_qr().await.unwrap();
    tokio::time::timeout(WAIT, phone.node.pairing_join(&uri.to_uri())).await.unwrap().expect("paired");
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    wait_for(pc, "paired", |e| match e {
        NodeEvent::Pairing(PairingEvent::Paired(d)) if d.id == phone_id => Some(()),
        _ => None,
    })
    .await;
    wait_for(&mut phone, "peer online", |e| match e {
        NodeEvent::LinkChanged { device, link: LinkState::Online { .. } } if *device == pc_id => Some(()),
        _ => None,
    })
    .await;
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    // `agent_push` is off until the PC's user turns it on, whatever the platform said.
    wait_catalog(&mut phone, pc_id, false).await;
    phone
}

/// A PC with the agent host paired with a phone (`agent_push` with `push`).
async fn pair(push: bool) -> (Device, Device) {
    let mut pc = start("Desktop", DeviceKind::Desktop, &[agent::HOST, agent::CONTROL]).await;
    pc.node.set_agent_catalog(Some(catalog())).await.unwrap();
    let mut phone = add_phone(&mut pc, "Pixel").await;
    if push {
        pc.node.set_device_toggle(phone.node.device_id(), git::PUSH_TOGGLE, true).unwrap();
        wait_catalog(&mut phone, pc.node.device_id(), true).await;
    }
    (pc, phone)
}

async fn timed<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("timed out: {what}"))
}

/// Reads `paths` of `review` whole on `phone`.
async fn read_all(phone: &Device, pc: DeviceId, review: &str, paths: &[&str]) {
    for path in paths {
        let patch = timed(
            "patch",
            phone.node.agent_git_patch(pc, THREAD.into(), (*path).into(), Some(review.into())),
        )
        .await
        .unwrap();
        assert!(!patch.truncated, "{path} came whole");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_op_reaches_the_host_and_its_answer_fits_the_phone() {
    let (pc, phone) = pair(true).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let status = timed("status", phone.node.agent_git_status(pc_id, THREAD.into())).await.unwrap();
    assert_eq!(status.files.len(), git::MAX_FILES, "a phone gets the first 500");
    assert!(status.more);
    assert_eq!(status.total, 700);
    let paths: Vec<&str> = status.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(&paths[..3], [SMALL, DOCS, "gen/f0.rs"], "no path out of the folder");
    assert_eq!(status.remotes.len(), 3);
    assert_eq!(status.remotes[0].label.as_deref(), Some("github.com/acme/app"));
    assert_eq!(status.remotes[1].label, None, "a URL never reaches a phone");
    assert_eq!((status.remotes[2].label.as_deref(), status.remotes[2].local), (None, true));
    assert_eq!(status.worktree, None, "nor a worktree's path");
    let sent = format!(
        "{:?}",
        (
            &status.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
            &status.remotes.iter().map(|r| &r.label).collect::<Vec<_>>()
        )
    );
    for secret in ["://", "hunter2", "/home/me", "/srv/git", "worktrees", "/etc/"] {
        assert!(!sent.contains(secret), "{secret} reached the phone");
    }
    let strip = phone.node.agent_cached_git(pc_id, THREAD.into()).await.unwrap();
    assert_eq!(strip.value.as_ref().map(|s| (s.changed, s.ahead, s.behind)), Some((700, Some(2), Some(1))));
    assert!(strip.saved_ms.is_some());

    let patch = phone.node.agent_git_patch(pc_id, THREAD.into(), SMALL.into(), None).await.unwrap();
    assert_eq!(patch.patch, "@@ -1 +1 @@\n-a\n+b\n", "the host's pages, joined");
    let folder = phone.node.agent_git_patch(pc_id, THREAD.into(), DOCS.into(), None).await.unwrap();
    assert_eq!(folder.names, ["docs/a.md", "docs/b.md"]);
    assert_eq!(folder.more_names, 1);

    let review =
        timed("review", phone.node.agent_git_review(pc_id, THREAD.into(), vec![SMALL.into(), OTHER.into()]))
            .await
            .unwrap();
    assert_ne!(review.review, HOST_REVIEW, "the phone gets a token of the PC's core, not the host's id");
    assert!(review.files.iter().all(|f| f.readable));
    read_all(&phone, pc_id, &review.review, &[SMALL, OTHER]).await;
    let commit = phone
        .node
        .agent_git_commit(
            pc_id,
            THREAD.into(),
            review.review.clone(),
            " Fix the retry\r\n\r\nIt waited.\u{0} ".into(),
        )
        .await
        .unwrap();
    assert_eq!((commit.kind.as_str(), commit.phase.as_str(), commit.mine), ("commit", "running", true));
    let AgentRequest::GitCommit { paths, message, review: host_review, .. } =
        pc.host.commits().pop().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(paths, [SMALL, OTHER], "the review's own paths");
    assert_eq!(host_review, HOST_REVIEW, "the host's review id");
    assert_eq!(message, "Fix the retry\n\nIt waited.", "cleaned");

    let push = phone.node.agent_git_push(pc_id, THREAD.into(), None).await.unwrap();
    assert_eq!(push.kind, "push");
    let fetch = phone.node.agent_git_fetch(pc_id, THREAD.into()).await.unwrap();
    assert_eq!((fetch.phase.as_str(), fetch.mine), ("done", false), "this PC asked for that one");
    let switch =
        phone.node.agent_git_switch(pc_id, THREAD.into(), "origin/feat/x".into(), false, true).await.unwrap();
    assert_eq!(switch.kind, "switch");
    let read = phone.node.agent_git_job(pc_id, JOB.into()).await.unwrap();
    assert_eq!(read.lines.len(), git::MAX_JOB_LINES, "a phone gets the newest 40 lines");
    assert_eq!((read.lines[0].as_str(), read.from, read.dropped), ("hook line 360", 360, 360));
    let stopped = phone.node.agent_git_stop(pc_id, JOB.into()).await.unwrap();
    assert_eq!(stopped.phase, "stopped");

    let branches = phone.node.agent_git_branches(pc_id, THREAD.into(), Some(" feat\n".into())).await.unwrap();
    assert_eq!(branches.branches.len(), git::MAX_BRANCHES);
    assert!(branches.more);
    assert_eq!(branches.branches[1].worktree, None, "a worktree's path is no label");

    let view = phone.node.agent_pr_view(pc_id, THREAD.into(), true).await.unwrap();
    assert_eq!(view.pr.as_ref().map(|p| p.checks.len()), Some(pr::MAX_CHECKS));
    let request = AgentPrCreateRequest {
        thread: THREAD.into(),
        title: "Fix the retry".into(),
        body: "It waited forever.\n\n- now it doesn't".into(),
        base: Some("main".into()),
        draft: true,
    };
    let opened = phone.node.agent_pr_create(pc_id, request).await.unwrap();
    assert_eq!(opened.kind, "pr_create");
    let rows = phone
        .node
        .agent_pr_list(
            pc_id,
            AgentPrListRequest { show: "folder".into(), folder: Some(FOLDER.into()), refresh: true },
        )
        .await
        .unwrap();
    assert_eq!(rows.prs.len(), pr::MAX_ROWS);
    assert!(rows.more);

    let asked = phone.node.agent_write(pc_id, THREAD.into(), "commit".into()).await.unwrap();
    assert_eq!(asked.wrote.map(|w| (w.purpose, w.text)), Some(("commit".into(), "Fix the retry".into())));

    let asked: Vec<AgentRequest> = pc.host.asked();
    let query = asked.iter().find_map(|r| match r {
        AgentRequest::GitBranches { query, .. } => Some(query.clone()),
        _ => None,
    });
    assert_eq!(query, Some(Some("feat".into())), "one line, trimmed");
    assert!(asked.iter().any(|r| matches!(r, AgentRequest::GitSwitch { track: true, create: false, .. })));
    assert!(asked.iter().any(|r| matches!(r, AgentRequest::PrCreate { draft: true, .. })));
    assert!(asked.iter().any(|r| matches!(r, AgentRequest::Ask { purpose, .. } if purpose == "commit")));
    assert!(pc.host.requests().iter().all(|(from, _)| *from == phone_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_change_and_a_jobs_lines_reach_the_phone() {
    let (pc, mut phone) = pair(false).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let change = AgentGitChanged {
        thread: THREAD.into(),
        branch: Some("feat/sync-retry".into()),
        changed: 3,
        ahead: Some(1),
        pr: Some(AgentPrBrief {
            number: 212,
            state: "open".into(),
            pass: 5,
            pending: 2,
            ..Default::default()
        }),
        ..Default::default()
    };
    pc.node.agent_git_changed(change.clone()).await.unwrap();
    let got = wait_for(&mut phone, "agent.git", |e| match e {
        NodeEvent::AgentGit { device, change } if *device == pc_id => Some(change.clone()),
        _ => None,
    })
    .await;
    assert_eq!(got, change);
    let saved = phone.node.agent_cached_git(pc_id, THREAD.into()).await.unwrap();
    assert_eq!(saved.value, Some(change.clone()), "kept for reading offline");

    // A status read later keeps the pull request the event brought.
    phone.node.agent_git_status(pc_id, THREAD.into()).await.unwrap();
    let saved = phone.node.agent_cached_git(pc_id, THREAD.into()).await.unwrap().value.unwrap();
    assert_eq!((saved.changed, saved.pr.map(|p| p.number)), (700, Some(212)));

    pc.node.agent_git_progress(job(git::kind::PUSH, git::phase::RUNNING, 100), Some(phone_id)).await.unwrap();
    let got = wait_for(&mut phone, "agent.git.progress", |e| match e {
        NodeEvent::AgentGitJob { device, job } if *device == pc_id => Some(job.clone()),
        _ => None,
    })
    .await;
    assert!(got.mine, "this phone asked for it");
    assert_eq!(got.lines.len(), git::MAX_JOB_LINES);
    assert_eq!(got.lines.last().map(String::as_str), Some("hook line 99"));
    pc.node.agent_git_progress(job(git::kind::PUSH, git::phase::DONE, 0), None).await.unwrap();
    let got = wait_for(&mut phone, "the job's end", |e| match e {
        NodeEvent::AgentGitJob { job, .. } if job.phase == "done" => Some(job.clone()),
        _ => None,
    })
    .await;
    assert!(!got.mine, "the PC's own");

    let bad = AgentGitChanged { thread: "Not An Id".into(), ..Default::default() };
    assert!(pc.node.agent_git_changed(bad).await.is_err());
    assert!(pc.node.agent_git_progress(job("rebase", "running", 0), None).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn push_and_pull_requests_need_agent_push_read_each_time() {
    let (pc, mut phone) = pair(false).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let create =
        || AgentPrCreateRequest { thread: THREAD.into(), title: "Fix it".into(), ..Default::default() };
    assert!(matches!(phone.node.agent_git_push(pc_id, THREAD.into(), None).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_pr_create(pc_id, create()).await, Err(Error::Denied)));
    assert!(pc.host.requests().is_empty(), "the host never heard of them: {:?}", pc.host.asked());

    pc.node.set_device_toggle(phone_id, git::PUSH_TOGGLE, true).unwrap();
    wait_catalog(&mut phone, pc_id, true).await;
    phone.node.agent_git_push(pc_id, THREAD.into(), Some("origin".into())).await.unwrap();
    phone.node.agent_pr_create(pc_id, create()).await.unwrap();
    assert_eq!(pc.host.requests().len(), 2);

    pc.node.set_device_toggle(phone_id, git::PUSH_TOGGLE, false).unwrap();
    wait_catalog(&mut phone, pc_id, false).await;
    assert!(matches!(phone.node.agent_git_push(pc_id, THREAD.into(), None).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_pr_create(pc_id, create()).await, Err(Error::Denied)));
    assert_eq!(pc.host.requests().len(), 2, "turned off between two pushes: the second never went");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_goes_only_after_every_file_was_read_whole_on_this_phone() {
    let (mut pc, phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    let commit = |review: &str, message: &str| {
        let node = &phone.node;
        let (review, message) = (review.to_owned(), message.to_owned());
        async move { node.agent_git_commit(pc_id, THREAD.into(), review, message).await }
    };

    // Without a review, the phone's core doesn't send it, and the PC refuses one sent raw.
    let raw = serde_json::json!({ "thread": THREAD, "message": "Fix it" });
    assert!(matches!(
        phone.node.debug_agent_request(pc_id, types::AGENT_GIT_COMMIT, &raw).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(commit("not-a-review", "Fix it").await, Err(Error::Protocol(_))));
    // A review no one made.
    assert!(matches!(commit("1111111111111111", "Fix it").await, Err(Error::NotFound)));

    let review = phone
        .node
        .agent_git_review(pc_id, THREAD.into(), vec![SMALL.into(), ENDLESS.into(), HUGE.into()])
        .await
        .unwrap();
    let huge = review.files.iter().find(|f| f.path == HUGE).unwrap();
    assert_eq!((huge.readable, huge.why.as_deref()), (false, Some("large")));
    let token = review.review.clone();
    // Nothing read yet; then one file; then the endless one, only ever half
    // served; and a file too large is never the phone's to commit.
    assert!(matches!(commit(&token, "Fix it").await, Err(Error::Denied)));
    read_all(&phone, pc_id, &token, &[SMALL]).await;
    assert!(matches!(commit(&token, "Fix it").await, Err(Error::Denied)));
    let half =
        phone.node.agent_git_patch(pc_id, THREAD.into(), ENDLESS.into(), Some(token.clone())).await.unwrap();
    assert!(half.truncated);
    assert!(matches!(commit(&token, "Fix it").await, Err(Error::Denied)));
    let outside = phone.node.agent_git_patch(pc_id, THREAD.into(), DOCS.into(), Some(token.clone())).await;
    assert!(matches!(outside, Err(Error::Protocol(_))), "a file the review doesn't hold");
    assert!(pc.host.commits().is_empty());

    // Read whole: it commits, once; a refusal before anything ran leaves the review.
    let review =
        phone.node.agent_git_review(pc_id, THREAD.into(), vec![SMALL.into(), OTHER.into()]).await.unwrap();
    let token = review.review;
    read_all(&phone, pc_id, &token, &[SMALL, OTHER]).await;
    let busy = commit(&token, BUSY_MESSAGE).await.unwrap();
    assert_eq!((busy.phase.as_str(), busy.error.map(|e| e.reason)), ("failed", Some("thread_busy".into())));
    commit(&token, "Fix the retry").await.unwrap();
    assert!(matches!(commit(&token, "Fix the retry").await, Err(Error::NotFound)), "one review commits once");
    assert_eq!(pc.host.commits().len(), 2);

    // Another phone can't use this phone's review.
    let review = phone.node.agent_git_review(pc_id, THREAD.into(), vec![SMALL.into()]).await.unwrap();
    read_all(&phone, pc_id, &review.review, &[SMALL]).await;
    let other = add_phone(&mut pc, "Tablet").await;
    let theirs =
        other.node.agent_git_commit(pc_id, THREAD.into(), review.review.clone(), "Fix it".into()).await;
    assert!(matches!(theirs, Err(Error::NotFound)), "{theirs:?}");
    let read =
        other.node.agent_git_patch(pc_id, THREAD.into(), SMALL.into(), Some(review.review.clone())).await;
    assert!(matches!(read, Err(Error::NotFound)));

    // Ten minutes on, the PC no longer keeps it.
    pc.node.debug_age_agent_reviews(Duration::from_millis(git::REVIEW_LIFE_MS));
    assert!(matches!(commit(&review.review, "Fix it").await, Err(Error::NotFound)));
    assert_eq!(pc.host.commits().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untracked_folder_is_never_committed_from_a_phone() {
    let (pc, phone) = pair(false).await;
    let pc_id = pc.node.device_id();
    // A folder of three files: the phone may see their names, never their bytes.
    let review =
        phone.node.agent_git_review(pc_id, THREAD.into(), vec![SMALL.into(), DOCS.into()]).await.unwrap();
    let docs = review.files.iter().find(|f| f.path == DOCS).unwrap();
    assert_eq!((docs.readable, docs.why.as_deref()), (false, Some("folder")));
    assert!(review.files.iter().find(|f| f.path == SMALL).is_some_and(|f| f.readable));
    read_all(&phone, pc_id, &review.review, &[SMALL, DOCS]).await;
    let names = phone
        .node
        .agent_git_patch(pc_id, THREAD.into(), DOCS.into(), Some(review.review.clone()))
        .await
        .unwrap();
    assert_eq!(names.names, ["docs/a.md", "docs/b.md"], "its names came, all of them");
    let commit =
        phone.node.agent_git_commit(pc_id, THREAD.into(), review.review.clone(), "Add docs".into()).await;
    assert!(matches!(commit, Err(Error::Denied)), "{commit:?}");
    // A review of the folder alone can't commit it either.
    let alone = phone.node.agent_git_review(pc_id, THREAD.into(), vec![DOCS.into()]).await.unwrap();
    read_all(&phone, pc_id, &alone.review, &[DOCS]).await;
    let commit = phone.node.agent_git_commit(pc_id, THREAD.into(), alone.review, "Add docs".into()).await;
    assert!(matches!(commit, Err(Error::Denied)), "{commit:?}");
    assert!(pc.host.commits().is_empty(), "nothing reached the host");
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_phone_may_not_do_never_reaches_the_host() {
    let (pc, phone) = pair(true).await;
    let pc_id = pc.node.device_id();
    let ask = |t: &'static str, body: serde_json::Value| {
        let node = &phone.node;
        async move { node.debug_agent_request(pc_id, t, &body).await }
    };
    // A switch that takes changes along is the PC's.
    let carry = serde_json::json!({ "thread": THREAD, "branch": "main", "carry": true });
    assert!(matches!(ask(types::AGENT_GIT_SWITCH, carry).await, Err(Error::Denied)));
    // No message says merge, ready, a worktree, or keep in step: they fail closed.
    let body = serde_json::json!({ "thread": THREAD, "number": 212, "method": "squash", "id": "w1" });
    for t in [
        "agent.git.merge",
        "agent.pr.merge",
        "agent.pr.ready",
        "agent.pr.methods",
        "agent.worktree.remove",
        "agent.worktree.list",
        "agent.folders.set",
        "agent.git.carry",
    ] {
        assert!(ask(t, body.clone()).await.is_err(), "{t}");
    }
    // Every limit.
    let long = |n: usize| "x".repeat(n);
    let cases = [
        (
            types::AGENT_GIT_COMMIT,
            serde_json::json!({ "thread": THREAD, "review": JOB, "message": long(8193) }),
        ),
        (types::AGENT_GIT_COMMIT, serde_json::json!({ "thread": THREAD, "review": JOB, "message": " \n " })),
        (
            types::AGENT_GIT_REVIEW,
            serde_json::json!({ "thread": THREAD, "paths": (0..201).map(|i| format!("f{i}")).collect::<Vec<_>>() }),
        ),
        (types::AGENT_GIT_REVIEW, serde_json::json!({ "thread": THREAD, "paths": [] })),
        (types::AGENT_GIT_REVIEW, serde_json::json!({ "thread": THREAD, "paths": ["d/".repeat(256) + "x"] })),
        (types::AGENT_GIT_REVIEW, serde_json::json!({ "thread": THREAD, "paths": ["/etc/passwd"] })),
        (types::AGENT_GIT_PATCH, serde_json::json!({ "thread": THREAD, "path": "../up" })),
        (types::AGENT_PR_CREATE, serde_json::json!({ "thread": THREAD, "title": long(257) })),
        (
            types::AGENT_PR_CREATE,
            serde_json::json!({ "thread": THREAD, "title": "Fix", "body": long(32 * 1024 + 1) }),
        ),
        (types::AGENT_GIT_SWITCH, serde_json::json!({ "thread": THREAD, "branch": long(256) })),
        (types::AGENT_GIT_SWITCH, serde_json::json!({ "thread": THREAD, "branch": "--force" })),
        (types::AGENT_GIT_PUSH, serde_json::json!({ "thread": THREAD, "remote": "--mirror" })),
        (types::AGENT_GIT_BRANCHES, serde_json::json!({ "thread": THREAD, "query": long(101) })),
        (types::AGENT_PR_LIST, serde_json::json!({ "show": "everyone" })),
        (types::AGENT_WRITE, serde_json::json!({ "thread": THREAD, "purpose": "merge" })),
        (types::AGENT_GIT_JOB, serde_json::json!({ "job": "nope" })),
    ];
    for (t, body) in cases {
        assert!(matches!(ask(t, body.clone()).await, Err(Error::Protocol(_))), "{t} {body}");
    }
    assert!(pc.host.requests().is_empty(), "nothing reached the host: {:?}", pc.host.asked());

    // The phone's own core doesn't send them either.
    assert!(phone.node.agent_git_commit(pc_id, THREAD.into(), JOB.into(), long(8193)).await.is_err());
    assert!(phone.node.agent_git_switch(pc_id, THREAD.into(), long(256), false, false).await.is_err());
    assert!(phone.node.agent_git_switch(pc_id, THREAD.into(), "x".into(), true, true).await.is_err());
    let many = (0..201).map(|i| format!("f{i}")).collect();
    assert!(phone.node.agent_git_review(pc_id, THREAD.into(), many).await.is_err());
    assert!(pc.host.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_agents_off_refuses_everything_and_forgets_the_reviews() {
    let (pc, phone) = pair(true).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());
    let review = phone.node.agent_git_review(pc_id, THREAD.into(), vec![SMALL.into()]).await.unwrap();
    read_all(&phone, pc_id, &review.review, &[SMALL]).await;
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, false).unwrap();
    assert!(matches!(phone.node.agent_git_status(pc_id, THREAD.into()).await, Err(Error::Denied)));
    assert!(matches!(phone.node.agent_git_fetch(pc_id, THREAD.into()).await, Err(Error::Denied)));
    let before = pc.host.requests().len();
    pc.node.set_device_toggle(phone_id, agent::TOGGLE, true).unwrap();
    let again = phone.node.agent_git_commit(pc_id, THREAD.into(), review.review, "Fix it".into()).await;
    assert!(matches!(again, Err(Error::NotFound)), "{again:?}");
    assert_eq!(pc.host.requests().len(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_requests_detail_and_diff_fit_the_phone_and_a_review_is_asked() {
    let (pc, phone) = pair(false).await;
    let (pc_id, phone_id) = (pc.node.device_id(), phone.node.device_id());

    let request =
        AgentPrDetailRequest { repo: Some("acme/app".into()), number: Some(212), ..Default::default() };
    let detail = timed("detail", phone.node.agent_pr_detail(pc_id, request)).await.unwrap();
    assert_eq!(detail.pr.as_ref().map(|p| p.number), Some(212));
    assert_eq!(detail.author, "octocat");
    assert!(detail.body.starts_with("Fixes the retry.[2J\n"));
    assert!(detail.body_cut && detail.body.len() == pr::MAX_BODY_BYTES);
    assert_eq!(detail.files.len(), pr::MAX_FILES, "the first 300, across the parts");
    assert!(detail.files_more);
    assert_eq!(detail.files[0].path, SMALL);
    assert!(detail.files.iter().all(|f| !f.path.contains("..")), "no path out of the repository");
    assert_eq!(detail.comments.len(), pr::MAX_COMMENTS, "a phone gets the newest 50");
    assert_eq!(detail.comments[0].author, "rev50");
    assert_eq!(detail.comments[49].review.as_deref(), Some("approved"));
    assert_eq!(detail.comments_older, 53);
    assert_eq!(detail.thread.as_deref(), Some(THREAD));

    let by_thread = AgentPrDetailRequest { thread: Some(THREAD.into()), refresh: true, ..Default::default() };
    timed("detail of a thread", phone.node.agent_pr_detail(pc_id, by_thread)).await.unwrap();

    let diff =
        timed("diff", phone.node.agent_pr_diff(pc_id, "acme/app".into(), 212, SMALL.into())).await.unwrap();
    assert_eq!(diff.patch, "diff --git a/src/sync.rs b/src/sync.rs\n-a\n+b\n", "both pages, joined");
    assert!(!diff.truncated && diff.names.is_empty());
    let endless = timed("endless", phone.node.agent_pr_diff(pc_id, "acme/app".into(), 212, ENDLESS.into()))
        .await
        .unwrap();
    assert!(endless.truncated && endless.patch.len() <= git::MAX_PATCH_BYTES);

    let asked = timed("ask", phone.node.agent_ask_review(pc_id, THREAD.into(), 212)).await.unwrap();
    assert_eq!(asked.id, THREAD);
    assert_eq!(asked.wrote, None, "a review writes nothing back");

    let requests = pc.host.requests();
    assert!(requests.iter().all(|(from, _)| *from == phone_id));
    let asked: Vec<AgentRequest> = requests.into_iter().map(|(_, r)| r).collect();
    assert!(asked.iter().any(|r| matches!(r,
        AgentRequest::PrDetail { thread: None, repo: Some(repo), number: Some(212), refresh: false } if repo == "acme/app")));
    assert!(asked.iter().any(|r| matches!(r,
        AgentRequest::PrDetail { thread: Some(t), repo: None, number: None, refresh: true } if t == THREAD)));
    assert!(
        asked
            .iter()
            .any(|r| matches!(r, AgentRequest::PrDiff { offset: Some(40), path, .. } if path == SMALL))
    );
    assert!(
        asked.iter().any(|r| matches!(r, AgentRequest::AskReview { thread, pr: 212 } if thread == THREAD))
    );
    assert!(!asked.iter().any(|r| matches!(r, AgentRequest::Ask { .. })), "a review isn't \"write it\"");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_request_out_of_its_limits_never_reaches_the_host() {
    let (pc, phone) = pair(true).await;
    let pc_id = pc.node.device_id();
    let ask = |t: &'static str, body: serde_json::Value| {
        let node = &phone.node;
        async move { node.debug_agent_request(pc_id, t, &body).await }
    };
    let cases = [
        (types::AGENT_PR_DETAIL, serde_json::json!({})),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "repo": "acme/app" })),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "number": 212 })),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "repo": "acme/app", "number": 0 })),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "repo": "-x/y", "number": 1 })),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "repo": "a/b/c/d", "number": 1 })),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "repo": "x".repeat(141), "number": 1 })),
        (types::AGENT_PR_DETAIL, serde_json::json!({ "thread": "/home/me" })),
        (types::AGENT_PR_DIFF, serde_json::json!({ "repo": "acme/app", "number": 212, "path": "../x" })),
        (
            types::AGENT_PR_DIFF,
            serde_json::json!({ "repo": "acme/app", "number": 212, "path": "d/".repeat(257) }),
        ),
        (types::AGENT_PR_DIFF, serde_json::json!({ "repo": "../x", "number": 212, "path": "a.rs" })),
        (types::AGENT_PR_DIFF, serde_json::json!({ "repo": "acme/app", "number": 0, "path": "a.rs" })),
        (types::AGENT_WRITE, serde_json::json!({ "thread": THREAD, "purpose": "review" })),
        (types::AGENT_WRITE, serde_json::json!({ "thread": THREAD, "purpose": "review", "pr": 0 })),
        (types::AGENT_WRITE, serde_json::json!({ "thread": THREAD, "purpose": "commit", "pr": 212 })),
    ];
    for (t, body) in cases {
        assert!(matches!(ask(t, body.clone()).await, Err(Error::Protocol(_))), "{t} {body}");
    }
    // Nothing posts: no message comments, reviews, approves or starts a review thread.
    let body = serde_json::json!({ "repo": "acme/app", "number": 212, "body": "LGTM" });
    for t in ["agent.pr.review", "agent.pr.comment", "agent.pr.approve", "agent.pr.start", "agent.pr.close"] {
        assert!(ask(t, body.clone()).await.is_err(), "{t}");
    }
    assert!(pc.host.requests().is_empty(), "nothing reached the host: {:?}", pc.host.asked());

    // The phone's own core doesn't send them either.
    let bad = AgentPrDetailRequest { repo: Some("-x/y".into()), number: Some(1), ..Default::default() };
    assert!(phone.node.agent_pr_detail(pc_id, bad).await.is_err());
    assert!(phone.node.agent_pr_diff(pc_id, "acme/app".into(), 212, "/etc/passwd".into()).await.is_err());
    assert!(phone.node.agent_ask_review(pc_id, THREAD.into(), 0).await.is_err());
    assert!(pc.host.requests().is_empty());

    // With the `agents` toggle off, they're refused before the host hears of them.
    pc.node.set_device_toggle(phone.node.device_id(), agent::TOGGLE, false).unwrap();
    let request = AgentPrDetailRequest { thread: Some(THREAD.into()), ..Default::default() };
    assert!(matches!(phone.node.agent_pr_detail(pc_id, request).await, Err(Error::Denied)));
    let diff = phone.node.agent_pr_diff(pc_id, "acme/app".into(), 212, SMALL.into()).await;
    assert!(matches!(diff, Err(Error::Denied)), "{diff:?}");
    let review = phone.node.agent_ask_review(pc_id, THREAD.into(), 212).await;
    assert!(matches!(review, Err(Error::Denied)), "{review:?}");
    assert!(pc.host.requests().is_empty());
}
