//! Approval observations shared by every frontend. Counts never imply eligibility.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Unapproved,
    NeedsCodeOwner,
    NeedsApprovals,
    Approved,
    NotRequired,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRuleKind {
    AnyApprover,
    Regular,
    CodeOwner,
    ReportApprover,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ApprovalRule {
    pub id: String,
    pub name: String,
    pub kind: ApprovalRuleKind,
    pub approvals_required: Option<u32>,
    /// Distinct qualifying approvers, not all users who reviewed the PR/MR.
    pub approvals_received: Option<u32>,
    pub approvals_remaining: Option<u32>,
    pub satisfied: Option<bool>,
}

/// One host observation. `None` counts/rules mean unavailable; zero/empty mean
/// confirmed zero/empty. Aggregate counts must not be summed from overlapping
/// rules. A stale observation is for display only and cannot match a predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ApprovalSummary {
    pub status: ApprovalStatus,
    pub requirements_satisfied: Option<bool>,
    pub approvals_received: Option<u32>,
    pub approvals_required: Option<u32>,
    pub approvals_remaining: Option<u32>,
    pub rules: Option<Vec<ApprovalRule>>,
    pub observed_at: DateTime<Utc>,
    #[serde(default)]
    pub stale: bool,
    /// Review head associated with this observation; absent when unavailable.
    pub head_sha: Option<String>,
}

impl ApprovalSummary {
    /// A stale observation matches only the explicit unknown condition.
    pub fn current_status(&self) -> ApprovalStatus {
        if self.stale {
            ApprovalStatus::Unknown
        } else {
            self.status
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_counts_are_distinct_from_confirmed_zero() {
        let summary = ApprovalSummary {
            status: ApprovalStatus::Unknown,
            requirements_satisfied: None,
            approvals_received: Some(0),
            approvals_required: None,
            approvals_remaining: None,
            rules: None,
            observed_at: Utc::now(),
            stale: false,
            head_sha: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["approvals_received"], 0);
        assert!(json["approvals_required"].is_null());
        assert!(json["rules"].is_null());
        assert_eq!(
            serde_json::from_value::<ApprovalSummary>(json).unwrap(),
            summary
        );
    }
}
