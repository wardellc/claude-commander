//! Host approval observations and conservative normalization.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use claude_commander_protocol::approval::{
    ApprovalRule, ApprovalRuleKind, ApprovalStatus, ApprovalSummary,
};
use serde_json::Value;
use tokio::process::Command;

use super::bounded::{self, Bounded};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(10);

fn gitlab_approval_command(repo_path: &Path, number: u32, suffix: &str) -> Command {
    let mut cmd = super::spawn::detached("glab");
    cmd.args([
        "api",
        &format!("projects/:fullpath/merge_requests/{number}{suffix}"),
        "--method",
        "GET",
    ])
    .env("GLAB_NO_PROMPT", "1")
    .current_dir(repo_path);
    cmd
}

pub(super) async fn gitlab_approvals(
    repo_path: &Path,
    number: u32,
    head_sha: Option<&str>,
) -> Option<ApprovalSummary> {
    gitlab_from_commands(
        gitlab_approval_command(repo_path, number, "/approvals"),
        gitlab_approval_command(repo_path, number, "/approval_state"),
        gitlab_approval_command(repo_path, number, ""),
        head_sha?,
    )
    .await
}

async fn gitlab_from_commands(
    summary: Command,
    details: Command,
    head: Command,
    expected_head: &str,
) -> Option<ApprovalSummary> {
    let (summary, details) = tokio::join!(api_json(summary), api_json(details));
    if summary.is_none() && details.is_none() {
        return None;
    }
    // Bracket the independent approval calls with the polling head and a fresh
    // MR read. A concurrent push makes this observation unusable for advancement.
    let confirmation = api_json(head).await?;
    if confirmation["sha"].as_str()? != expected_head {
        return None;
    }
    if summary
        .as_ref()
        .and_then(|v| v["sha"].as_str())
        .is_some_and(|sha| sha != expected_head)
    {
        return None;
    }
    // GitLab doc/api/merge_requests.md, "Merge status": approval syncing is
    // unsettled; requested_changes identifies blocking reviewer feedback.
    match confirmation["detailed_merge_status"].as_str() {
        Some("approvals_syncing" | "preparing") => None,
        Some("requested_changes") => {
            let mut result =
                gitlab_summary(summary.as_ref(), details.as_ref(), Some(expected_head));
            result.status = ApprovalStatus::Blocked;
            result.requirements_satisfied = Some(false);
            Some(result)
        }
        _ => Some(gitlab_summary(
            summary.as_ref(),
            details.as_ref(),
            Some(expected_head),
        )),
    }
}

pub(super) async fn api_json(cmd: Command) -> Option<Value> {
    let Bounded::Finished { status, stdout, .. } =
        bounded::run_bounded(cmd, "approval query", APPROVAL_TIMEOUT)
            .await
            .ok()?
    else {
        return None;
    };
    if !status.success() {
        return None;
    }
    serde_json::from_slice(&stdout).ok()
}

fn count(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

fn distinct_approvers(value: &Value, nested: bool) -> Option<u32> {
    let mut ids = BTreeSet::new();
    for entry in value.as_array()? {
        let user = if nested { &entry["user"] } else { entry };
        let id = user["id"]
            .as_u64()
            .map(|id| id.to_string())
            .or_else(|| user["username"].as_str().map(str::to_owned))?;
        ids.insert(id);
    }
    u32::try_from(ids.len()).ok()
}

fn gitlab_rule(value: &Value) -> Option<ApprovalRule> {
    let kind = match value["rule_type"].as_str() {
        Some("any_approver") => ApprovalRuleKind::AnyApprover,
        Some("regular") => ApprovalRuleKind::Regular,
        Some("code_owner") => ApprovalRuleKind::CodeOwner,
        Some("report_approver") => ApprovalRuleKind::ReportApprover,
        _ => ApprovalRuleKind::Other,
    };
    let required = count(&value["approvals_required"]);
    let received = distinct_approvers(&value["approved_by"], false);
    let satisfied = value["approved"].as_bool();
    // Numerical deficits apply to user approval rules, not security/report rules.
    // GitLab's approval_state receipt: doc/api/merge_request_approvals.md,
    // "Retrieve approval details for a merge request" (rule-specific approved_by).
    let remaining = match kind {
        ApprovalRuleKind::Regular | ApprovalRuleKind::AnyApprover | ApprovalRuleKind::CodeOwner => {
            required
                .zip(received)
                .map(|(required, received)| required.saturating_sub(received))
        }
        _ => None,
    };
    Some(ApprovalRule {
        id: value["id"].as_u64()?.to_string(),
        name: value["name"].as_str()?.to_owned(),
        kind,
        approvals_required: required,
        approvals_received: received,
        approvals_remaining: remaining,
        satisfied,
    })
}

pub(super) fn gitlab_summary(
    summary: Option<&Value>,
    details: Option<&Value>,
    head_sha: Option<&str>,
) -> ApprovalSummary {
    let empty = Value::Null;
    let summary = summary.unwrap_or(&empty);
    let rules = details
        .and_then(|v| v["rules"].as_array())
        .and_then(|rules| rules.iter().map(gitlab_rule).collect::<Option<Vec<_>>>());
    let received = distinct_approvers(&summary["approved_by"], true);
    let required = count(&summary["approvals_required"]);
    let remaining = count(&summary["approvals_left"]);
    let rule_satisfaction = rules.as_ref().and_then(|rules| {
        if rules
            .iter()
            .any(|r| r.satisfied == Some(false) && r.approvals_required != Some(0))
        {
            Some(false)
        } else if rules
            .iter()
            .all(|r| r.satisfied == Some(true) || r.approvals_required == Some(0))
        {
            Some(true)
        } else {
            None
        }
    });
    let aggregate = summary["approved"].as_bool();
    let conflicting = aggregate
        .zip(rule_satisfaction)
        .is_some_and(|(a, b)| a != b)
        || (aggregate == Some(true) && remaining.is_some_and(|n| n > 0));
    let satisfied = if conflicting {
        None
    } else {
        rule_satisfaction.or(aggregate)
    };
    let status = if conflicting {
        ApprovalStatus::Unknown
    } else if satisfied == Some(true) {
        if received == Some(0) {
            if required == Some(0)
                || rules
                    .as_ref()
                    .is_some_and(|r| r.iter().all(|r| r.approvals_required == Some(0)))
            {
                ApprovalStatus::NotRequired
            } else {
                ApprovalStatus::Unknown
            }
        } else if received.is_some_and(|n| n > 0)
            || required.is_some_and(|n| n > 0)
            || rules.as_ref().is_some_and(|rules| {
                rules
                    .iter()
                    .any(|r| r.approvals_required.is_some_and(|n| n > 0))
            })
        {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Unknown
        }
    } else if satisfied == Some(false) && received == Some(0) {
        ApprovalStatus::Unapproved
    } else if satisfied == Some(false) && received.is_some_and(|n| n > 0) {
        match rules.as_ref() {
            Some(rules)
                if rules.iter().any(|r| {
                    r.kind == ApprovalRuleKind::CodeOwner
                        && r.satisfied == Some(false)
                        && r.approvals_required != Some(0)
                }) =>
            {
                ApprovalStatus::NeedsCodeOwner
            }
            Some(rules)
                if rules
                    .iter()
                    .any(|r| r.satisfied == Some(false) && r.approvals_required != Some(0)) =>
            {
                ApprovalStatus::NeedsApprovals
            }
            _ => ApprovalStatus::Unknown,
        }
    } else {
        ApprovalStatus::Unknown
    };
    ApprovalSummary {
        status,
        requirements_satisfied: satisfied,
        approvals_received: received,
        approvals_required: required,
        approvals_remaining: remaining,
        rules,
        observed_at: Utc::now(),
        stale: false,
        head_sha: head_sha.map(str::to_owned),
    }
}

const GITHUB_APPROVAL_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){headRefOid reviewDecision latestOpinionatedReviews(first:100){totalCount nodes{author{login} state}}}}}";

pub(super) async fn github_approvals(
    repo_path: &Path,
    pr_url: &str,
    number: u32,
) -> Option<ApprovalSummary> {
    let url = url::Url::parse(pr_url).ok()?;
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let [owner, name, "pull", ..] = segments.as_slice() else {
        return None;
    };
    let mut cmd = super::gh_command();
    cmd.args([
        "api",
        "graphql",
        "--hostname",
        url.host_str()?,
        "-f",
        &format!("query={GITHUB_APPROVAL_QUERY}"),
        "-f",
        &format!("owner={owner}"),
        "-f",
        &format!("name={name}"),
        "-F",
        &format!("number={number}"),
    ])
    .current_dir(repo_path);
    let response = api_json(cmd).await?;
    if response.get("errors").is_some() {
        return None;
    }
    github_summary(&response["data"]["repository"]["pullRequest"])
}

fn github_summary(value: &Value) -> Option<ApprovalSummary> {
    let head_sha = value["headRefOid"].as_str()?.to_owned();
    let reviews = &value["latestOpinionatedReviews"];
    let received = reviews["nodes"].as_array().and_then(|nodes| {
        if count(&reviews["totalCount"])? as usize != nodes.len() {
            return None;
        }
        let mut approvers = BTreeSet::new();
        for review in nodes {
            match review["state"].as_str()? {
                "APPROVED" => {
                    approvers.insert(review["author"]["login"].as_str()?.to_lowercase());
                }
                "CHANGES_REQUESTED" | "DISMISSED" | "COMMENTED" | "PENDING" => {}
                _ => return None,
            }
        }
        u32::try_from(approvers.len()).ok()
    });
    let (status, satisfied) = match value["reviewDecision"].as_str() {
        Some("APPROVED") => (ApprovalStatus::Approved, Some(true)),
        Some("CHANGES_REQUESTED") => (ApprovalStatus::Blocked, Some(false)),
        Some("REVIEW_REQUIRED") if received == Some(0) => (ApprovalStatus::Unapproved, Some(false)),
        Some("REVIEW_REQUIRED") => (ApprovalStatus::Unknown, Some(false)),
        _ => (ApprovalStatus::Unknown, None),
    };
    // Use GitHub's opinionated-review connection, excluding comment-only
    // participation. Receipt: querying both latestReviews and
    // latestOpinionatedReviews on cli/cli#14784 (2026-10-21) returns COMMENTED
    // entries only in the former. Count only a complete connection below.
    // Required counts/reasons remain unknown: branch protection alone cannot
    // describe additional repository/organization rulesets.
    Some(ApprovalSummary {
        status,
        requirements_satisfied: satisfied,
        approvals_received: received,
        approvals_required: None,
        approvals_remaining: None,
        rules: None,
        observed_at: Utc::now(),
        stale: false,
        head_sha: Some(head_sha),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_commander_protocol::approval::{ApprovalRuleKind, ApprovalStatus};
    use serde_json::json;

    fn summary(received: usize, approved: bool) -> serde_json::Value {
        json!({"approved": approved, "approvals_required": 2,
            "approvals_left": if approved { 0 } else { 1 },
            "approved_by": (0..received).map(|id| json!({"user":{"id":id}})).collect::<Vec<_>>()})
    }

    fn rule(
        id: u32,
        kind: &str,
        required: u32,
        received: usize,
        approved: bool,
    ) -> serde_json::Value {
        json!({"id":id,"name":format!("rule {id}"),"rule_type":kind,
            "approvals_required":required,"approved":approved,
            "approved_by":(0..received).map(|id|json!({"id":id})).collect::<Vec<_>>()})
    }

    #[test]
    fn enough_total_approvals_can_still_need_a_code_owner() {
        let data = json!({"rules":[rule(1,"regular",2,2,true),rule(2,"code_owner",1,0,false)]});
        let result = gitlab_summary(Some(&summary(2, false)), Some(&data), Some("head"));
        assert_eq!(result.status, ApprovalStatus::NeedsCodeOwner);
        assert_eq!(result.approvals_received, Some(2));
        assert_eq!(result.approvals_remaining, Some(1));
        let rules = result.rules.unwrap();
        assert_eq!(rules[1].kind, ApprovalRuleKind::CodeOwner);
        assert_eq!(rules[1].approvals_remaining, Some(1));
    }

    #[test]
    fn multiple_required_people_are_not_just_a_total_count() {
        for required in [1, 2] {
            let data = json!({"rules":[rule(1,"regular",required,0,false)]});
            let result = gitlab_summary(Some(&summary(2, false)), Some(&data), None);
            assert_eq!(result.status, ApprovalStatus::NeedsApprovals);
            assert_eq!(result.requirements_satisfied, Some(false));
        }
    }

    #[test]
    fn every_required_owner_rule_must_be_satisfied() {
        for (second_approved, expected) in [
            (false, ApprovalStatus::NeedsCodeOwner),
            (true, ApprovalStatus::Approved),
        ] {
            let rules = json!({"rules":[rule(1,"code_owner",1,1,true),rule(2,"code_owner",1,usize::from(second_approved),second_approved)]});
            let aggregate = summary(2, second_approved);
            assert_eq!(
                gitlab_summary(Some(&aggregate), Some(&rules), None).status,
                expected
            );
        }
        assert_eq!(
            gitlab_summary(None, Some(&json!({"rules":[]})), None).status,
            ApprovalStatus::Unknown
        );
    }

    #[test]
    fn overlapping_rules_are_never_summed() {
        let data = json!({"rules":[rule(1,"regular",2,1,false),rule(2,"code_owner",2,1,false)]});
        let mut aggregate = summary(1, false);
        aggregate
            .as_object_mut()
            .unwrap()
            .remove("approvals_required");
        aggregate.as_object_mut().unwrap().remove("approvals_left");
        let result = gitlab_summary(Some(&aggregate), Some(&data), None);
        assert_eq!(result.approvals_required, None);
        assert_eq!(result.approvals_remaining, None);
    }

    #[test]
    fn unknown_and_zero_and_no_rules_have_different_meanings() {
        assert_eq!(
            gitlab_summary(None, None, None).status,
            ApprovalStatus::Unknown
        );
        let data =
            json!({"approved":true,"approvals_required":0,"approvals_left":0,"approved_by":[]});
        assert_eq!(
            gitlab_summary(Some(&data), Some(&json!({"rules":[]})), None).status,
            ApprovalStatus::NotRequired
        );
        assert_eq!(
            gitlab_summary(
                Some(&json!({"approved":false,"approved_by":[]})),
                None,
                None
            )
            .status,
            ApprovalStatus::Unapproved
        );
        assert_eq!(
            gitlab_summary(Some(&json!({"approved":true,"approved_by":[]})), None, None).status,
            ApprovalStatus::Unknown
        );
        assert!(gitlab_summary(None, Some(&json!({})), None).rules.is_none());
    }

    #[test]
    fn duplicate_approvers_are_counted_once_and_malformed_lists_are_unknown() {
        let data = json!({"approved":false,"approved_by":[{"user":{"id":1}},{"user":{"id":1}}]});
        assert_eq!(
            gitlab_summary(Some(&data), None, None).approvals_received,
            Some(1)
        );
        let data = json!({"approved_by":[{}]});
        assert_eq!(
            gitlab_summary(Some(&data), None, None).approvals_received,
            None
        );
    }

    #[test]
    fn conflicting_snapshots_and_unknown_rules_cannot_approve() {
        let rules = json!({"rules":[rule(1,"code_owner",1,0,false)]});
        assert_eq!(
            gitlab_summary(Some(&summary(2, true)), Some(&rules), None).status,
            ApprovalStatus::Unknown
        );
        let rules = json!({"rules":[{"id":1,"name":"new rule","rule_type":"future"}]});
        assert_eq!(
            gitlab_summary(None, Some(&rules), None).requirements_satisfied,
            None
        );
        assert_eq!(
            gitlab_summary(None, Some(&json!({"rules":[{}]})), None).status,
            ApprovalStatus::Unknown
        );
    }

    #[test]
    fn github_uses_opinionated_reviews_and_never_invents_required_counts() {
        let data = json!({"headRefOid":"abc","reviewDecision":"APPROVED",
            "latestOpinionatedReviews":{"totalCount":3,"nodes":[
                {"author":{"login":"alice"},"state":"APPROVED"},
                {"author":{"login":"bob"},"state":"DISMISSED"},
                {"author":{"login":"carol"},"state":"COMMENTED"}]}});
        let result = github_summary(&data).unwrap();
        assert_eq!(result.status, ApprovalStatus::Approved);
        assert_eq!(result.approvals_received, Some(1));
        assert_eq!(result.approvals_required, None);
        assert_eq!(result.approvals_remaining, None);
        assert_eq!(result.rules, None);
    }

    #[test]
    fn github_incomplete_reviews_do_not_become_zero_and_changes_block() {
        let data = json!({"headRefOid":"abc","reviewDecision":"REVIEW_REQUIRED",
            "latestOpinionatedReviews":{"totalCount":101,"nodes":[]}});
        let result = github_summary(&data).unwrap();
        assert_eq!(result.status, ApprovalStatus::Unknown);
        assert_eq!(result.approvals_received, None);
        let data = json!({"headRefOid":"abc","reviewDecision":"CHANGES_REQUESTED"});
        assert_eq!(
            github_summary(&data).unwrap().status,
            ApprovalStatus::Blocked
        );
    }

    fn json_command(value: serde_json::Value) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf '%s' \"$1\"", "fixture", &value.to_string()]);
        cmd
    }

    #[tokio::test]
    async fn gitlab_unavailable_rules_keep_authoritative_summary_and_pushes_invalidate_it() {
        let mut denied = Command::new("sh");
        denied.args(["-c", "exit 1"]);
        let result = gitlab_from_commands(
            json_command(summary(2, true)),
            denied,
            json_command(json!({"sha":"head"})),
            "head",
        )
        .await
        .unwrap();
        assert_eq!(result.status, ApprovalStatus::Approved);
        assert!(result.rules.is_none());
        assert!(
            gitlab_from_commands(
                json_command(summary(2, true)),
                json_command(json!({"rules":[]})),
                json_command(json!({"sha":"new-head"})),
                "head"
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn gitlab_syncing_is_unknown_and_requested_changes_block_approval() {
        for (merge_status, status) in [
            ("approvals_syncing", None),
            ("requested_changes", Some(ApprovalStatus::Blocked)),
        ] {
            let result = gitlab_from_commands(
                json_command(summary(2, true)),
                json_command(json!({"rules":[]})),
                json_command(json!({"sha":"head","detailed_merge_status":merge_status})),
                "head",
            )
            .await;
            assert_eq!(result.map(|s| s.status), status);
        }
    }

    #[test]
    fn gitlab_approval_queries_are_repository_local_and_noninteractive() {
        let cmd = gitlab_approval_command(Path::new("/repo"), 17, "/approval_state");
        assert_eq!(
            cmd.as_std().get_args().collect::<Vec<_>>(),
            [
                "api",
                "projects/:fullpath/merge_requests/17/approval_state",
                "--method",
                "GET"
            ]
        );
        assert_eq!(cmd.as_std().get_current_dir(), Some(Path::new("/repo")));
        assert!(
            cmd.as_std()
                .get_envs()
                .any(|(k, v)| k == "GLAB_NO_PROMPT" && v == Some(std::ffi::OsStr::new("1")))
        );
    }
}
