// SPDX-License-Identifier: GPL-3.0-or-later
//! Phones' source-control requests (`docs/protocol/agents.md` §8, contract
//! §SC6.1), after the core checked the toggles, the limits and the review:
//! each is passed to the host as this phone (`by`), and what comes back is
//! cut to what a phone may get (no folder on this PC, a worktree by its
//! label and branch, at most 500 files, a job's newest 40 lines). A switch
//! never takes changes along; merge, Mark ready, worktrees and "keep in
//! step" have no request at all. A pull request's detail and diff (§13) are
//! read-only; "Ask <agent> to review it" sends the host's own message.

use g7_agents::api::{
    GitBranchesParams, GitChangedEvent, GitCommitParams, GitFetchParams, GitJobParams, GitPatchParams,
    GitPushParams, GitReviewParams, GitStatus, GitSwitchParams, Job, PrCreateParams, PrDetailParams,
    PrDetailResult, PrDiffParams, PrInfo, PrListParams, PrShow, PrViewParams, PrViewResult, Purpose,
};
use g7link_core::{
    AgentBranch, AgentBranches, AgentCheck, AgentCheckCounts, AgentGhState, AgentGit, AgentGitChanged,
    AgentGitFile, AgentGitHub, AgentGitLast, AgentGitPatchPage, AgentGitRemote, AgentGitReviewed,
    AgentGitReviewedFile, AgentGitWorktree, AgentJob, AgentJobError, AgentJobResult, AgentPrBrief,
    AgentPrComment, AgentPrDetail, AgentPrFile, AgentPrInfo, AgentPrRow, AgentPrRows, AgentPrView,
};

use super::*;

// ---- The host's records as a phone gets them ----

fn wire_status(st: &GitStatus) -> AgentGit {
    let st = source::phone_status(st);
    AgentGit {
        repo: word(&st.repo),
        branch: st.branch,
        detached: st.detached,
        unborn: st.unborn,
        upstream: st.upstream,
        ahead: st.ahead,
        behind: st.behind,
        remote: st.remote,
        remotes: st
            .remotes
            .into_iter()
            .map(|r| AgentGitRemote { name: r.name, label: (!r.local).then_some(r.label), local: r.local })
            .collect(),
        default_branch: st.default_branch,
        on_default: st.on_default,
        doing: st.doing.as_ref().map(word),
        locked: st.locked,
        files: st
            .files
            .into_iter()
            .map(|f| AgentGitFile {
                status: Some(word(&f.status)),
                staged: Some(word(&f.staged)),
                path: f.path,
                from: f.from,
                folder: f.folder,
                adds: f.adds,
                dels: f.dels,
                binary: f.binary,
            })
            .collect(),
        more: st.more,
        total: st.total,
        adds: st.adds,
        dels: st.dels,
        conflicts: st.conflicts,
        last: st.last.map(|l| AgentGitLast { oid7: l.oid7, subject: l.subject, ms: l.ms }),
        worktree: st.worktree.map(|w| AgentGitWorktree { label: w.label, branch: w.branch }),
        fetched_ms: st.fetched_ms,
        checked_ms: st.checked_ms,
        github: st.github.map(|g| AgentGitHub { repo: g.repo, host: g.host }),
        fingerprint: st.fingerprint,
    }
}

fn wire_pr(pr: &PrInfo) -> AgentPrInfo {
    AgentPrInfo {
        number: pr.number,
        title: pr.title.clone(),
        state: word(&pr.state),
        draft: pr.draft,
        url: pr.url.clone(),
        base: pr.base.clone(),
        head: pr.head.clone(),
        review: pr.review.as_ref().map(word),
        mergeable: pr.mergeable.as_ref().map(word),
        blocked: pr.blocked,
        updated_ms: pr.updated_ms,
        checks: pr
            .checks
            .iter()
            .map(|c| AgentCheck { name: c.name.clone(), state: word(&c.state), url: c.url.clone() })
            .collect(),
        pass: pr.pass,
        fail: pr.fail,
        pending: pr.pending,
        read_ms: pr.read_ms,
    }
}

fn wire_gh(gh: &api::GhState) -> AgentGhState {
    AgentGhState {
        gh: word(&gh.gh),
        version: gh.version.clone(),
        hosts: gh.hosts.clone(),
        checked_ms: gh.checked_ms,
    }
}

fn wire_view(view: &PrViewResult) -> AgentPrView {
    AgentPrView {
        pr: view.pr.as_ref().map(wire_pr),
        gh: wire_gh(&view.gh),
        github: view.github.as_ref().map(|g| AgentGitHub { repo: g.repo.clone(), host: g.host.clone() }),
        why: view.why.as_ref().map(word),
    }
}

/// A job as a phone gets it (its newest 40 lines); `mine` is the core's to set.
pub(super) fn wire_job(job: &Job) -> AgentJob {
    let job = source::phone_job(job);
    AgentJob {
        kind: word(&job.kind),
        phase: word(&job.phase),
        id: job.id,
        thread: job.thread_id,
        started_ms: job.started_ms,
        ended_ms: job.ended_ms,
        mine: false,
        lines: job.lines,
        from: job.from,
        dropped: job.dropped,
        result: job.result.map(|r| AgentJobResult {
            oid7: r.oid7,
            subject: r.subject,
            files: r.files,
            branch: r.branch,
            remote: r.remote,
            up_to_date: r.up_to_date,
            created: r.created,
            pr: r.pr.as_ref().map(wire_pr),
            number: r.number,
        }),
        error: job.error.map(|e| AgentJobError { reason: e.reason, message: e.message, names: e.names }),
    }
}

/// The device a job is for (`None`: this PC).
pub(super) fn job_by(job: &Job) -> Option<DeviceId> {
    (job.by.kind == OriginKind::Phone).then(|| job.by.device_id.as_deref()?.parse().ok()).flatten()
}

/// `git.changed` as the phones' strip.
pub(super) fn wire_changed(e: &GitChangedEvent) -> AgentGitChanged {
    AgentGitChanged {
        thread: e.thread_id.clone(),
        branch: e.branch.clone(),
        detached: e.detached,
        changed: e.changed,
        ahead: e.ahead,
        behind: e.behind,
        pr: e.pr.as_ref().map(|p| AgentPrBrief {
            number: p.number,
            state: word(&p.state),
            draft: p.draft,
            pass: p.pass,
            fail: p.fail,
            pending: p.pending,
        }),
    }
}

/// A pull request's detail as a phone gets it: no folder of this PC; the
/// core cuts the comments to the phone's bounds.
pub(super) fn wire_detail(result: &PrDetailResult) -> AgentPrDetail {
    let d = result.detail.as_ref();
    AgentPrDetail {
        pr: d.map(|d| wire_pr(&d.pr)),
        gh: wire_gh(&result.gh),
        why: result.why.as_ref().map(word),
        repo: d.map(|d| d.repo.clone()).unwrap_or_default(),
        author: d.map(|d| d.author.clone()).unwrap_or_default(),
        body: d.map(|d| d.body.clone()).unwrap_or_default(),
        body_cut: d.is_some_and(|d| d.body_cut),
        adds: d.map_or(0, |d| d.adds),
        dels: d.map_or(0, |d| d.dels),
        changed: d.map_or(0, |d| d.changed),
        files: d.map_or_else(Vec::new, |d| {
            d.files
                .iter()
                .map(|f| AgentPrFile {
                    path: f.path.clone(),
                    change: f.change.clone(),
                    adds: f.adds,
                    dels: f.dels,
                })
                .collect()
        }),
        files_more: d.is_some_and(|d| d.files_more),
        comments: d.map_or_else(Vec::new, |d| {
            d.comments
                .iter()
                .map(|c| AgentPrComment {
                    author: c.author.clone(),
                    at_ms: c.at_ms,
                    body: c.body.clone(),
                    cut: c.cut,
                    review: c.review.clone(),
                    url: c.url.clone(),
                })
                .collect()
        }),
        comments_older: d.map_or(0, |d| d.comments_older),
        thread: d.and_then(|d| d.thread_id.clone()),
        read_ms: d.map_or(0, |d| d.read_ms),
    }
}

/// The host's refusal with a reason word (`data.reason`) as the core words
/// it; any other error as a failure.
pub(super) fn refused(e: ClientError) -> Result<AgentReply, AgentFailure> {
    match e {
        ClientError::Remote(rpc) if rpc.data.as_ref().and_then(|d| d.get("reason")).is_some() => {
            let data = rpc.data.unwrap_or_default();
            let reason = data.get("reason").and_then(Value::as_str).unwrap_or_default();
            Ok(AgentReply::Refused(AgentJobError {
                reason: reason.chars().take(24).collect(),
                message: api::cut(&rpc.message, 200).to_owned(),
                names: Vec::new(),
            }))
        }
        e => Err(failure(&e)),
    }
}

/// A request that starts a job: the job, or the host's reason when it
/// refused before anything ran.
fn job_reply(result: Result<Job, ClientError>, from: DeviceId) -> Result<AgentReply, AgentFailure> {
    match result {
        Ok(job) => Ok(AgentReply::GitJob { job: wire_job(&job), by: Some(from) }),
        Err(ClientError::Remote(rpc)) if rpc.data.as_ref().and_then(|d| d.get("reason")).is_some() => {
            let data = rpc.data.unwrap_or_default();
            let reason = data.get("reason").and_then(Value::as_str).unwrap_or_default();
            let names = data
                .get("names")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter().filter_map(Value::as_str).take(api::MAX_REMOTES).map(str::to_owned).collect()
                })
                .unwrap_or_default();
            Ok(AgentReply::Refused(AgentJobError {
                reason: reason.chars().take(24).collect(),
                message: api::cut(&rpc.message, 200).to_owned(),
                names,
            }))
        }
        Err(e) => Err(failure(&e)),
    }
}

/// Answers a phone's source-control request.
pub(super) async fn phone(
    from: DeviceId,
    client: &Client,
    request: AgentRequest,
) -> Result<AgentReply, AgentFailure> {
    let fail = |e: ClientError| failure(&e);
    let by = || Some(Origin::phone(from.to_string(), device_name(&from)));
    Ok(match request {
        AgentRequest::GitStatus { thread } => {
            let params = api::GitStatusParams { thread_id: Some(thread), folder_id: None };
            AgentReply::Git(wire_status(&client.git_status(params).await.map_err(fail)?))
        }
        AgentRequest::GitPatch { thread, path, offset } => {
            let page =
                client.git_patch(GitPatchParams { thread_id: thread, path, offset }).await.map_err(fail)?;
            AgentReply::GitPatch(AgentGitPatchPage {
                patch: page.patch,
                next: page.next,
                truncated: page.truncated,
                binary: page.binary,
                names: page.names,
                more_names: page.more_names,
            })
        }
        AgentRequest::GitReview { thread, paths } => {
            let review = client
                .git_review(GitReviewParams { thread_id: thread, paths, all: false })
                .await
                .map_err(fail)?;
            AgentReply::Review(AgentGitReviewed {
                review: review.review,
                files: review
                    .files
                    .into_iter()
                    .map(|f| AgentGitReviewedFile {
                        path: f.path,
                        bytes: f.bytes,
                        folder: f.folder,
                        entries: f.entries,
                    })
                    .collect(),
                pc_only: review.pc_only,
            })
        }
        AgentRequest::GitCommit { thread, paths, message, review } => {
            let params = GitCommitParams {
                thread_id: thread,
                paths,
                all: false,
                message,
                review: Some(review),
                by: by(),
            };
            return job_reply(client.git_commit(params).await, from);
        }
        AgentRequest::GitPush { thread, remote } => {
            return job_reply(
                client.git_push(GitPushParams { thread_id: thread, remote, by: by() }).await,
                from,
            );
        }
        AgentRequest::GitFetch { thread } => {
            return job_reply(client.git_fetch(GitFetchParams { thread_id: thread, by: by() }).await, from);
        }
        AgentRequest::GitBranches { thread, query } => {
            let params = GitBranchesParams { thread_id: Some(thread), folder_id: None, query };
            let list = client.git_branches(params).await.map_err(fail)?;
            AgentReply::Branches(AgentBranches {
                branches: list
                    .branches
                    .into_iter()
                    .map(|b| AgentBranch {
                        name: b.name,
                        remote: b.remote,
                        current: b.current,
                        upstream: b.upstream,
                        ahead: b.ahead,
                        behind: b.behind,
                        worktree: b.worktree,
                        last_ms: b.last_ms,
                    })
                    .collect(),
                more: list.more,
                current: list.current,
            })
        }
        AgentRequest::GitSwitch { thread, branch, create, track } => {
            // A phone never takes changes along: the host refuses a dirty folder.
            let params = GitSwitchParams { thread_id: thread, branch, create, track, carry: false, by: by() };
            return job_reply(client.git_switch(params).await, from);
        }
        AgentRequest::GitJob { job } => {
            let job = client.git_job(job).await.map_err(fail)?;
            AgentReply::GitJob { by: job_by(&job), job: wire_job(&job) }
        }
        AgentRequest::GitStop { job } => {
            let job = client.git_stop(GitJobParams { job, by: by() }).await.map_err(fail)?;
            AgentReply::GitJob { by: job_by(&job), job: wire_job(&job) }
        }
        AgentRequest::PrView { thread, refresh } => AgentReply::Pr(wire_view(
            &client.pr_view(PrViewParams { thread_id: thread, refresh }).await.map_err(fail)?,
        )),
        AgentRequest::PrCreate { thread, title, body, base, draft } => {
            let params = PrCreateParams { thread_id: thread, title, body, base, draft, by: by() };
            return job_reply(client.pr_create(params).await, from);
        }
        AgentRequest::PrList { show, folder, refresh } => {
            let show = match show.as_str() {
                "mine" => PrShow::Mine,
                "review" => PrShow::Review,
                "folder" => PrShow::Folder,
                _ => return Err(AgentFailure::BadMessage),
            };
            let list =
                client.pr_list(PrListParams { show, folder_id: folder, refresh }).await.map_err(fail)?;
            AgentReply::PrRows(AgentPrRows {
                prs: list
                    .prs
                    .into_iter()
                    .map(|p| AgentPrRow {
                        number: p.row.number,
                        title: p.row.title,
                        repo: p.row.repo,
                        state: word(&p.row.state),
                        draft: p.row.draft,
                        updated_ms: p.row.updated_ms,
                        url: p.row.url,
                        branch: p.row.branch,
                        checks: p.row.checks.map(|c| AgentCheckCounts {
                            pass: c.pass,
                            fail: c.fail,
                            pending: c.pending,
                        }),
                        thread: p.thread_id,
                    })
                    .collect(),
                more: list.more,
                gh: wire_gh(&list.gh),
            })
        }
        AgentRequest::Ask { thread, purpose } => {
            let purpose = match purpose.as_str() {
                "commit" => Purpose::Commit,
                "pr" => Purpose::Pr,
                _ => return Err(AgentFailure::BadMessage),
            };
            let origin = Origin::phone(from.to_string(), device_name(&from));
            client
                .thread_send_with(ThreadSendParams::ask(thread.clone(), purpose, origin))
                .await
                .map_err(fail)?;
            let params = ThreadReadParams { thread_id: thread, limit: Some(1), ..Default::default() };
            let read = client.thread_read(params).await.map_err(fail)?;
            AgentReply::Thread(super::thread(&read.thread, Some(&from)))
        }
        // §13: read-only; the host checks the pull request is the thread's,
        // or one it listed.
        AgentRequest::PrDetail { thread, repo, number, refresh } => {
            let params = PrDetailParams { thread_id: thread, repo, number, refresh };
            match client.pr_detail(params).await {
                Ok(result) => AgentReply::PrDetail(wire_detail(&result)),
                Err(e) => return refused(e),
            }
        }
        AgentRequest::PrDiff { repo, number, path, offset } => {
            let params = PrDiffParams { repo, number, path, offset, thread_id: None };
            match client.pr_diff(params).await {
                Ok(page) => AgentReply::GitPatch(AgentGitPatchPage {
                    patch: page.patch,
                    next: page.next,
                    truncated: page.truncated,
                    binary: page.binary,
                    names: Vec::new(),
                    more_names: 0,
                }),
                Err(e) => return refused(e),
            }
        }
        // The host writes the message from the number and a link it builds.
        AgentRequest::AskReview { thread, pr } => {
            let origin = Origin::phone(from.to_string(), device_name(&from));
            client
                .thread_send_with(ThreadSendParams::ask_review(thread.clone(), pr, origin))
                .await
                .map_err(fail)?;
            let params = ThreadReadParams { thread_id: thread, limit: Some(1), ..Default::default() };
            let read = client.thread_read(params).await.map_err(fail)?;
            AgentReply::Thread(super::thread(&read.thread, Some(&from)))
        }
        _ => return Err(AgentFailure::BadMessage),
    })
}

#[cfg(test)]
mod tests {
    use g7_agents::api::{GitChange, GitFile, RepoState, Staged};

    use super::*;

    #[test]
    fn a_phones_status_names_no_folder_on_this_pc() {
        let st = GitStatus {
            repo: RepoState::Ok,
            branch: Some("feat/x".into()),
            remotes: vec![
                api::Remote { name: "origin".into(), label: "github.com/o/r".into(), local: false },
                api::Remote { name: "here".into(), label: "/srv/git/r.git".into(), local: true },
            ],
            files: vec![GitFile {
                path: "a.rs".into(),
                from: None,
                status: GitChange::Untracked,
                staged: Staged::None,
                folder: false,
                adds: Some(1),
                dels: None,
                binary: false,
            }],
            worktree: Some(api::WorktreeTag {
                id: "abcdef012345".into(),
                branch: "g7/x".into(),
                label: "x".into(),
                missing: false,
            }),
            ..Default::default()
        };
        let wire = wire_status(&st);
        assert_eq!(wire.repo, "ok");
        assert_eq!(wire.files[0].status.as_deref(), Some("untracked"));
        assert_eq!(wire.remotes[0].label.as_deref(), Some("github.com/o/r"));
        assert_eq!(wire.remotes[1].label, None, "a folder on this PC isn't named");
        assert_eq!(wire.worktree.as_ref().map(|w| w.branch.as_str()), Some("g7/x"));
    }

    #[test]
    fn a_jobs_device_is_read_from_its_by() {
        let job: Job = serde_json::from_value(json!({
            "id": "0123456789abcdef", "thread_id": "t", "kind": "push", "phase": "running", "started_ms": 1,
            "by": { "kind": "phone", "device_id": "not a device id" },
        }))
        .expect("a job");
        assert_eq!(job_by(&job), None, "an id that doesn't parse names no device");
        let pc: Job = serde_json::from_value(json!({
            "id": "0123456789abcdef", "thread_id": "t", "kind": "push", "phase": "done", "started_ms": 1,
            "by": { "kind": "launcher" },
        }))
        .expect("a job");
        assert_eq!(job_by(&pc), None);
        assert_eq!(wire_job(&pc).kind, "push");
    }
}
