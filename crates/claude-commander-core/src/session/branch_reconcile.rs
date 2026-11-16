//! Keep a session anchored to its working branch across renames and continuation
//! from an unused starter. Existing branches stay anchored across temporary
//! switches. Git evidence is gathered by `starter_branch_continued`; the service
//! supplies remote-ref, cached-review, main-branch and sibling-ownership guards.

/// Decide whether a session's stored branch should be reconciled to `live`,
/// the worktree's current HEAD branch. Returns `Some(new_branch)` to adopt it,
/// or `None` to leave `session.branch` unchanged.
///
/// Pure: every git fact is passed in so this is exhaustively unit-testable.
///
/// - `stored` — the session's currently recorded branch.
/// - `live` — the worktree's live HEAD branch, or `None` when HEAD is detached,
///   the worktree is missing, or the branch could not be read.
/// - `stored_local_ref_exists` — whether `refs/heads/<stored>` still exists.
/// - `stored_remote_ref_exists` — whether `refs/remotes/origin/<stored>` exists.
/// - `main_branch` — the project's default branch.
/// - `sibling_branches` — stored branches of the *other* non-`Creating`
///   sessions in the same project.
///
/// Existing local branches are retained unless `starter_continuation` proves
/// committed work on a branch created from the initial commit during this session.
/// The caller only supplies that evidence for a starter without a cached review.
/// Remote branches, main, and branches owned by sibling sessions are always retained.
/// With neither old ref present, the existing rename behavior applies.
pub fn decide_branch_reconcile(
    stored: &str,
    live: Option<&str>,
    stored_local_ref_exists: bool,
    stored_remote_ref_exists: bool,
    main_branch: &str,
    sibling_branches: &[&str],
    starter_continuation: bool,
) -> Option<String> {
    let live = live?;
    if live == stored || live == "HEAD" {
        return None;
    }
    if stored_remote_ref_exists || (stored_local_ref_exists && !starter_continuation) {
        return None;
    }
    if live == main_branch {
        return None;
    }
    if sibling_branches.contains(&live) {
        return None;
    }
    Some(live.to_string())
}

/// Recognize committed work on a new branch from an unused session starter.
/// Missing reflogs, failed commands, and unrelated history are inconclusive.
pub(crate) async fn starter_branch_continued(
    worktree: &std::path::Path,
    stored: &str,
    live: &str,
    base: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    let stored_ref = format!("refs/heads/{stored}");
    let live_ref = format!("refs/heads/{live}");
    let Some(stored_tip) = git_text(worktree, &["rev-parse", "--verify", &stored_ref]).await else {
        return false;
    };
    if stored_tip.trim() != base {
        return false;
    }
    let Some(live_tip) = git_text(worktree, &["rev-parse", "--verify", &live_ref]).await else {
        return false;
    };
    if live_tip.trim() == base {
        return false;
    }
    if git_text(worktree, &["merge-base", "--is-ancestor", base, &live_ref])
        .await
        .is_none()
    {
        return false;
    }
    let Some(reflog) = git_text(
        worktree,
        &[
            "reflog",
            "show",
            "--date=unix",
            "--format=%H%x09%gD%x09%gs",
            &live_ref,
        ],
    )
    .await
    else {
        return false;
    };
    if !created_from_start(&reflog, base, created_at) {
        return false;
    }
    let Some(head_reflog) = git_text(
        worktree,
        &[
            "reflog",
            "show",
            "--date=unix",
            "--format=%H%x09%gD%x09%gs",
            "HEAD",
        ],
    )
    .await
    else {
        return false;
    };
    committed_in_worktree(
        &head_reflog,
        stored,
        live,
        base,
        live_tip.trim(),
        created_at,
    )
}

/// Require commits in this worktree since checking out the new branch at the
/// starter's base. The service's other-worktree regression uses real Git to pin
/// the distinction from merely switching to a branch with someone else's work.
fn committed_in_worktree(
    reflog: &str,
    stored: &str,
    live: &str,
    base: &str,
    tip: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    let mut committed = false;
    let expected_checkout = format!("checkout: moving from {stored} to {live}");
    for (index, entry) in reflog.lines().enumerate() {
        let Some((sha, timestamp, message)) = reflog_entry(entry) else {
            return false;
        };
        if index == 0 && sha != tip {
            return false;
        }
        if message.starts_with("checkout:") {
            return committed
                && sha == base
                && message == expected_checkout
                && timestamp > created_at.timestamp();
        }
        if !message.starts_with("commit:") && !message.starts_with("commit (") {
            return false;
        }
        committed = true;
    }
    false
}

async fn git_text(repo: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = crate::git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn created_from_start(reflog: &str, base: &str, created_at: chrono::DateTime<chrono::Utc>) -> bool {
    let Some((sha, timestamp, message)) = reflog.lines().last().and_then(reflog_entry) else {
        return false;
    };
    sha == base && message.starts_with("branch: Created from ")
        // Reflog dates have second precision, so same-second creation is ambiguous.
        && timestamp > created_at.timestamp()
}

fn reflog_entry(entry: &str) -> Option<(&str, i64, &str)> {
    let mut fields = entry.splitn(3, '\t');
    let (sha, selector, message) = (fields.next()?, fields.next()?, fields.next()?);
    let timestamp = selector
        .rsplit_once("@{")
        .and_then(|(_, time)| time.strip_suffix('}'))
        .and_then(|time| time.parse::<i64>().ok())?;
    Some((sha, timestamp, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_requires_a_complete_recent_creation_record() {
        let created = chrono::DateTime::from_timestamp(100, 500_000_000).unwrap();
        let valid = "base\trefs/heads/new@{101}\tbranch: Created from main";
        assert!(created_from_start(valid, "base", created));
        for invalid in [
            "",
            "malformed",
            "base\trefs/heads/new@{99}\tbranch: Created from main",
            "base\trefs/heads/new@{100}\tbranch: Created from main",
            "other\trefs/heads/new@{101}\tbranch: Created from main",
            "base\trefs/heads/new@{101}\tcommit: retained after reflog expiry",
            "base\trefs/heads/new@{bad}\tbranch: Created from main",
        ] {
            assert!(!created_from_start(invalid, "base", created), "{invalid}");
        }
    }

    #[test]
    fn continuation_preserves_remote_main_and_sibling_guards() {
        assert_eq!(
            decide_branch_reconcile("old", Some("new"), true, false, "main", &[], true),
            Some("new".into())
        );
        assert_eq!(
            decide_branch_reconcile("old", Some("new"), true, true, "main", &[], true),
            None
        );
        assert_eq!(
            decide_branch_reconcile("old", Some("main"), true, false, "main", &[], true),
            None
        );
        assert_eq!(
            decide_branch_reconcile("old", Some("new"), true, false, "main", &["new"], true),
            None
        );
    }

    // Convenience with the common "no refs, main is `main`, no siblings" case.
    fn decide(stored: &str, live: Option<&str>, local: bool, remote: bool) -> Option<String> {
        decide_branch_reconcile(stored, live, local, remote, "main", &[], false)
    }

    #[test]
    fn renamed_branch_is_adopted() {
        // Old name gone (no local, no remote), live differs → adopt.
        assert_eq!(decide("old", Some("new"), false, false), Some("new".into()));
    }

    #[test]
    fn unchanged_branch_is_left() {
        assert_eq!(decide("same", Some("same"), false, false), None);
    }

    #[test]
    fn detached_head_is_left() {
        assert_eq!(decide("old", Some("HEAD"), false, false), None);
    }

    #[test]
    fn missing_live_branch_is_left() {
        // Worktree not found / unreadable → no candidate.
        assert_eq!(decide("old", None, false, false), None);
    }

    #[test]
    fn plain_switch_is_left_when_stored_local_ref_exists() {
        // `git switch other` — the stored branch still exists locally, so this
        // is a transient switch, not a rename.
        assert_eq!(decide("old", Some("other"), true, false), None);
    }

    #[test]
    fn unpushed_rename_is_left_when_remote_ref_exists() {
        // Renamed locally but not pushed: GitHub's PR still heads `old`, and
        // `origin/old` still exists. Reconciling would wipe a working PR;
        // defer until the rename is pushed and `origin/old` disappears.
        assert_eq!(decide("old", Some("new"), false, true), None);
    }

    #[test]
    fn live_equal_to_main_is_left() {
        // Stored branch deleted, HEAD fell back to main → abandonment.
        assert_eq!(
            decide_branch_reconcile("old", Some("main"), false, false, "main", &[], false),
            None
        );
    }

    #[test]
    fn collision_with_sibling_is_left() {
        // Adopting `new` would make this session and the sibling both claim it.
        assert_eq!(
            decide_branch_reconcile("old", Some("new"), false, false, "main", &["new"], false),
            None
        );
    }

    #[test]
    fn adopt_when_sibling_set_does_not_collide() {
        assert_eq!(
            decide_branch_reconcile(
                "old",
                Some("new"),
                false,
                false,
                "main",
                &["unrelated"],
                false
            ),
            Some("new".into())
        );
    }
}
