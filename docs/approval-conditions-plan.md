# Approval-aware section conditions

Status: implemented baseline. GitLab supplies rule details and available host counts;
GitHub supplies the aggregate decision and complete current-review counts, with
required/remaining counts and precise missing-rule reasons left unavailable.
Count display in the UI remains a follow-up. The design and implementation
sequence below record the approved plan.

## Current behaviour

- The section condition is `review_decision`, not `is_approved`. It accepts
  `approved`, `review_required`, or `changes_requested`, either singly or as an
  any-of array. Existing conditions must retain their meaning.
- GitHub polling requests `reviewDecision` from `gh pr list` and maps it directly.
  We do not calculate approval from a count of submitted reviews. The protocol
  comment describing Approved as “at least one” is not evidence of GitHub's
  actual treatment of required approval rules.
- GitLab polling deliberately sets `review_decision` to `None`. Approval parity
  was deferred in `gitlab-cli-support-plan.md`.
- `has_reviewer` is assignment/review participation, not approval. GitHub includes
  requested reviewers and review authors; GitLab currently includes assigned
  reviewers. It cannot answer whether a required approver has approved.
- Section assignment is forward-only. Losing approval does not automatically
  move a session back out of a section it has already reached.

Implementation anchors: core's `git/pr.rs`, `git/hosting/gitlab.rs`,
`session/section.rs`, `session/types.rs`, and `api.rs`; protocol's `pr.rs` and
`api.rs`; the section editor in TUI `app/settings.rs`.

## Proposed public behaviour

Add `approval_status` rather than changing the type or meaning of
`review_decision`. Accept a scalar or an any-of array, like existing predicates.

- `unapproved`: no current approvals, with outstanding approval requirements.
- `needs_code_owner`: at least one current approval, but at least one required
  Code Owner rule remains unsatisfied. This takes precedence over
  `needs_approvals` when both apply.
- `needs_approvals`: at least one current approval, but other approval rules
  remain unsatisfied. This includes a numerical shortfall or a particular
  required person/group, even if the overall number of approvals looks sufficient.
- `approved`: the host confirms all applicable approval requirements are met,
  with at least one current approval.
- `not_required`: the host confirms no approval is required and none is present.
  Keep this distinct from an actual approval; do not infer it from absent data.
- `blocked`: an authoritative outstanding blocking review, where exposed by the
  provider. Do not classify CI failures or merge conflicts as approval blockers.
- `unknown`: insufficient, unsupported, inaccessible, stale, or inconsistent
  approval data. Unknown must not satisfy another approval predicate.

Blocking reviews take precedence over the other known states. An unresolved
required-person rule is classified as `needs_approvals`, unless it is explicitly
a Code Owner rule. Never infer Code Ownership from a requested reviewer's name.

If the boolean/string spelling is preferred, a new `is_approved` predicate could
provide `true` = `approved`, `false` = `unapproved`, and the named partial states.
Recommendation: use the explicit `approval_status` spelling. Here `false` would
not be the complement of `true`, which makes boolean configuration surprising.
There is no existing `is_approved` key to preserve in this checkout.

Example, in forward process order:

```toml
[[sections]]
name = "Awaiting approvals"
pr_state = "open"
approval_status = ["unapproved", "needs_approvals"]

[[sections]]
name = "Awaiting Code Owner"
pr_state = "open"
approval_status = "needs_code_owner"

[[sections]]
name = "Approved"
pr_state = "open"
is_draft = false
approval_status = "approved"
```

These columns represent process milestones under existing forward-only rules.
Show current approval status on the card/details so a milestone cannot conceal
a withdrawn approval. Automatic backwards movement would be a separate opt-in
feature, with its own migration and tests. “Approved” means approval requirements
are met; it does not promise successful CI or general mergeability.

## Counts exposed for future UI

Expose the approval summary in both session-list and session-detail API responses,
and carry it through the TypeScript and Flutter bindings. Counts are part of the
initial data contract, even if their visual presentation is implemented later.

Proposed summary fields:

- `approvals_received`: distinct users with current, non-dismissed approvals.
  This is participation, not proof that their approvals satisfy required rules.
- `approvals_required`: the host-reported aggregate required count, when available.
- `approvals_remaining`: the host-reported aggregate outstanding count, when
  available. Do not derive it by subtracting received from required: an approval
  may not satisfy any required rule.
- `rules`: optional per-rule details containing a stable host rule identifier,
  display name, rule kind (including Code Owner), required count, qualifying
  received count, remaining count when reliably known, and host-reported
  satisfaction. A missing rules list means unavailable; an empty list means the
  host confirmed there are no applicable rules.
- Data availability/freshness and observation time, so clients can distinguish
  current counts from a cached observation after a failed refresh.

All counts are optional nonnegative integers: `null` means unknown, while `0`
means a confirmed zero. Derive per-rule qualifying received counts only from a
complete rule-specific approver list, counting each user once. Derive per-rule
remaining counts only where the provider's rule semantics make that valid;
host-reported satisfaction remains authoritative if counts disagree or race.
Do not manufacture aggregate counts by summing rules: the same approval can
satisfy several rules. Do not infer aggregate satisfaction from the counts.

A future UI could display “1 of 2 approvals” with separate unmet-rule detail such
as “Code Owner approval required”. It must still show that requirement if the
total reaches “2 of 2”. Prefer “1 approval remaining” when that is the only known
aggregate count. Hide unavailable numbers instead of showing them as zero, and
mark cached counts as stale. These fields do not add a user-configured approval
threshold or change the host's approval policy.

## Provider data and limitations

### GitLab first

After selecting the MR, query its `/approvals` summary and, where available,
`/approval_state` rule details through detached, noninteractive `glab api` calls.
Use the selected MR's project identity, the repository's authenticated host,
bounded concurrency/timeouts, and the existing provider-switch race guard.

The summary distinguishes all approving users from rule satisfaction. Rule
details expose `rule_type`, `approvals_required`, `approved`, and `approved_by`.
Use the host's rule satisfaction, rather than recreating eligibility, hidden-group
membership, or Code Owner file matching locally. Preserve unmet reasons together:
the displayed classification must not discard a simultaneous count shortfall.
Do not sum per-rule deficits: one person's approval can satisfy several rules.

Edition/version support matters. The documented rule-details endpoint is
Premium/Ultimate. Current upstream docs also describe different semantics for
the summary's `approved` across Community and Enterprise editions. Missing rule
details must not turn into an empty, satisfied rule list. When the summary gives
an authoritative aggregate decision, retain it even if the precise missing reason
is unknown; otherwise classify as unknown.

### GitHub second

Keep using the host's `reviewDecision` for the existing condition. Before adding
exact reason classification, run a small fixture-repository investigation with
one/two required approvals, required Code Owner reviews, branch protection,
rulesets, stale/dismissed reviews, and the available token permissions.

Verify what the aggregate decision guarantees and which APIs reliably expose
the unmet requirements. Use an authoritative aggregate approval when verified;
return unknown for a missing reason that cannot be established. Do not label
every `REVIEW_REQUIRED` result as “no approvals” or “needs Code Owner”, or treat
general `mergeStateStatus = BLOCKED` as proof of an approval shortfall. Avoid a
local CODEOWNERS/team-membership engine in the initial implementation.

## Implementation sequence

1. Capture provider fixtures and write failing normalization/predicate tests.
   Include two non-owner approvals with an owner still required, one of two
   approvals, a named approver still missing despite enough total approvals,
   multiple Code Owner rules, overlapping rules, no rules, withdrawn approvals,
   permission failures, unsupported endpoints, and unknown/missing fields.
2. Add optional approval-summary wire types in protocol. Carry authoritative
   satisfaction separately from counts, unmet reasons, and data availability.
   Include the aggregate and per-rule counts specified above from the start.
   Use optional counts when unknown. Keep fetching and provider policy in core.
3. Add GitLab enrichment to the regular PR/MR polling path, not just the details
   view, so section assignment has the data. Bound additional calls. Approval
   enrichment failure must not discard otherwise valid MR metadata.
4. Persist optional cached approval data with serde defaults; clear it on MR
   removal, identity/head changes, and provider switches. Preserve previous
   observations on transient failure but mark them stale: stale approval cannot
   newly advance a session. Keep all existing persisted field names unchanged.
5. Add the predicate to matching, predicate detection, config persistence, and
   the settings editor. Propagate the optional summary through API snapshots,
   generated TypeScript, and Flutter mirrors/bridge glue. Use FRB codegen 2.11.1.
6. Complete the GitHub investigation and implement only classifications backed
   by its results. Document remaining provider/permission limitations explicitly.
7. Update configuration docs and badges/details. Test old configuration/state
   loading, provider-switch races, partial fetch failure, section ordering,
   manual pins, and approval loss after advancement. Test count serialization
   through list/detail responses, unknown versus zero, duplicate approvers,
   dismissed reviews, overlapping rules, and totals that appear sufficient while
   a required Code Owner or named approver is still missing.
8. Verify with the pinned toolchains using `CC_FORCE_NIX=1 scripts/verify.sh --all`.
   Obtain peer/Fable review before an implementation PR and follow repository
   PR/CI requirements.

## References checked

- [GitLab approvals API](https://docs.gitlab.com/api/merge_request_approvals/),
  including `/approvals` versus `/approval_state`. The docs site returned HTTP
  403 during investigation; its upstream
  [Markdown source](https://github.com/gitlabhq/gitlabhq/blob/master/doc/api/merge_request_approvals.md)
  was inspected instead. Check deployed GitLab versions before relying on newer
  summary fields such as `approval_rules_left`.
- [GitHub protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches):
  required review counts, required Code Owner review, and stale approval policy.
  These document enforcement requirements, not a verified mapping of every
  combination to GraphQL `reviewDecision`.
