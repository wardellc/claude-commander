# Claude Commander: GitHub and GitLab CLI Support Implementation Plan

## Objective

Add GitLab CLI (`glab`) support alongside the existing GitHub CLI (`gh`)
integration.

“Git” means ordinary local/distributed Git functionality and is out of scope. Do
not alter ordinary clone-by-URL, worktree, diff, fetch, push, branch, or merge
behavior except where needed to preserve existing functionality.

GitHub must remain the default and must behave exactly as before for users whose
config is unchanged.

Implement the full plan sequentially. It may be delivered as multiple reviewable
PRs, but each PR must be internally functional and its description must include
a concrete manual test checklist for the repository owner.

## Repository rules

Before changing anything:

1. Read the repository’s current `AGENTS.md`/`CLAUDE.md`.
2. Inspect the current branch and worktree; do not overwrite unrelated changes.
3. Treat paths and line numbers in this plan as guides because the repository may
   have moved since it was written.
4. For every behavior change:
   - Write a failing test first.
   - Implement the minimal fix.
   - Run the prescribed verification.
   - Obtain peer/Fable review and address findings.
5. Use `scripts/verify.sh`; do not improvise equivalent Cargo or Flutter command
   chains.
6. For generated bindings or golden changes, run verification with the pinned
   toolchain:

   ```sh
   CC_FORCE_NIX=1 scripts/verify.sh --all
   ```

7. Do not hand-edit Flutter Rust Bridge generated files.
8. Do not hand-edit generated screenshots.
9. No automated test may contact GitHub or GitLab or require an authenticated
   `gh`/`glab` account.

## Current runtime command inventory

There are seven direct GitHub CLI behaviors that need provider-aware equivalents:

| Capability | Existing GitHub command | GitLab implementation |
|---|---|---|
| CLI availability | `gh --version` | `glab version` |
| Repository listing | `gh api --paginate ... /user/repos` | `glab api projects ... --paginate --output ndjson` |
| Hosted clone | `gh repo clone -- <owner/name> <dest>` | `glab repo clone <namespace/path> <dest>` |
| PR/MR polling | `gh pr list --head ... --state all --json ... --limit 5` | GitLab MR API through `glab api` |
| Rich PR/MR detail | `gh pr view <number> --json ...` | GitLab MR API through `glab api` |
| Stack retarget | `gh pr edit <number> --base <branch>` | `glab mr update <iid> --target-branch <branch> --yes` |
| Agent stack instruction | `gh pr create --base <branch>` | `glab mr create --target-branch <branch>` |

Do not mechanically replace `gh` with `glab`; their argument layouts and response
fields differ.

## Pre-implementation CLI receipt

The implementing agent should record the installed CLI version and relevant help
text before pinning command-shape tests:

```sh
glab version
glab api --help
glab repo clone --help
glab mr update --help
glab mr create --help
```

If `glab` is unavailable in the agent environment, ask the repository owner to
run these commands and return the output. Do not run authenticated or mutating
commands merely to inspect help.

The intended commands are supported by the official documentation:

- <https://docs.gitlab.com/cli/version/>
- <https://docs.gitlab.com/cli/api/>
- <https://docs.gitlab.com/cli/repo/clone/>
- <https://docs.gitlab.com/cli/mr/update/>
- <https://docs.gitlab.com/cli/mr/create/>

## Fixed product decisions

Implement these decisions unless the repository owner explicitly changes them.

### Provider model

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CodeHostProvider {
    #[default]
    Github,
    Gitlab,
}
```

The shared provider type belongs in `claude-commander-protocol`, because the
server and clients must agree on it.

Add to `Config`:

```rust
#[serde(default)]
pub code_host_provider: CodeHostProvider,

#[serde(default, skip_serializing_if = "Option::is_none")]
pub gitlab_hostname: Option<String>,
```

Example:

```toml
# Applies to repository discovery, hosted clone, and all PR/MR integration.
code_host_provider = "github" # "github" or "gitlab"

# Optional; omitted means gitlab.com or glab's configured default.
# Used for repository discovery and hosted clone outside a repository.
# gitlab_hostname = "gitlab.example.com"
```

A missing provider must deserialize as GitHub.

### Global-provider limitation

Phase 1 uses one global provider for all hosted operations on a Commander host.

Document clearly:

> Claude Commander integrates with one selected code-host provider at a time.
> Selecting GitLab disables GitHub PR polling for registered GitHub projects,
> and selecting GitHub disables GitLab MR polling for registered GitLab projects.

Per-project provider detection is a future enhancement. Do not silently guess
unknown hosts.

### Self-managed GitLab

Self-managed GitLab is in scope.

- Repository listing uses `glab api --hostname <host>`.
- Hosted clone freezes the hostname into the request and applies it through
  `GITLAB_HOST`.
- Repository-local MR commands run in the project checkout and allow `glab` to
  resolve the authenticated host from the repository remote.
- Validate configured hostnames. Reject schemes, paths, control characters,
  option-shaped strings, and userinfo/credentials.

### Persisted PR fields

Keep the existing persisted `pr_*` field names and JSON shape in Phase 1.

Generalize their comments to mean code review / pull request / merge request. Do
not opportunistically rename persisted fields.

### GitLab review decision

For Phase 1:

- Preserve GitLab reviewer usernames.
- Set `review_decision` to `None`.
- Do not infer `ReviewRequired` merely because reviewers exist.
- Do not implement GitLab approval-state calls in this change.

Approval parity can be a later feature because GitLab editions and approval
rules do not map cleanly to GitHub’s `reviewDecision`.

### GitLab CI status

Normalize `head_pipeline.status` as:

| GitLab status | Normalized status |
|---|---|
| `success`, `skipped` | `Passing` |
| `failed`, `canceled` | `Failing` |
| `created`, `waiting_for_resource`, `preparing`, `pending`, `running`, `scheduled`, `manual`, `waiting_for_callback`, `canceling`, unknown non-empty status | `Pending` |
| Missing/null pipeline | `None` |

This is pipeline-level parity, not job/check-rollup parity.

## Target module structure

Keep hosting logic under `git` unless the current architecture gives a stronger
reason for another location:

```text
crates/claude-commander-core/src/git/hosting/
  mod.rs
  github.rs
  gitlab.rs
  process.rs       # only if shared subprocess helpers justify it
```

Responsibilities:

- `hosting/mod.rs`: provider-neutral facade and normalized runtime models.
- `hosting/github.rs`: existing `gh` commands and GitHub payload parsing.
- `hosting/gitlab.rs`: `glab` commands and GitLab payload parsing.
- `hosting/process.rs`: availability/noninteractive/bounded helpers if shared.

Prefer a simple provider dispatch over an elaborate trait if only two internal
adapters exist:

```rust
match provider {
    CodeHostProvider::Github => github::list_repositories(...).await,
    CodeHostProvider::Gitlab => gitlab::list_repositories(...).await,
}
```

Keep ordinary Git operations outside the hosting adapters.

## Protocol models

Create a provider-neutral protocol module such as:

```text
crates/claude-commander-protocol/src/hosting.rs
```

Update imports directly. Do not add Rust re-export shims for moved types.

### Hosting identity

```rust
struct CodeHost {
    provider: CodeHostProvider,
    hostname: Option<String>,
}
```

For GitHub, `hostname` is normally `None`. GitHub Enterprise discovery is not
being added here.

### Repository visibility

```rust
enum RepositoryVisibility {
    Public,
    Internal,
    Private,
}
```

Do not collapse GitLab `internal` into `private`.

### Repository DTO

```rust
struct HostedRepository {
    full_name: String,
    namespace: String,
    name: String,
    description: Option<String>,
    visibility: RepositoryVisibility,
    fork: bool,
    archived: bool,
    default_branch: Option<String>,
    clone_url: String,
    ssh_url: String,
    activity_at: Option<DateTime<Utc>>,
}
```

Notes:

- GitLab supports nested namespaces.
- `default_branch` must be optional because empty GitLab projects return null.
- `activity_at` intentionally normalizes different concepts:
  - GitHub: `pushed_at`
  - GitLab: `last_activity_at`
- Do not claim the timestamps have identical semantics.

### Repository-list response

Return an envelope:

```rust
struct RepositoryListing {
    host: CodeHost,
    repositories: Vec<HostedRepository>,
}
```

A provider field only on individual rows is insufficient because an empty result
would not tell the client which provider was queried.

### Clone source

Preserve the existing GitHub wire variant for compatibility and add GitLab:

```rust
#[serde(rename_all = "snake_case", tag = "kind")]
enum CloneSource {
    Github {
        full_name: String,
    },
    Gitlab {
        full_name: String,
        #[serde(default)]
        hostname: Option<String>,
    },
    Url {
        url: String,
    },
}
```

The GitLab hostname must be copied from the repository listing into the request.
Once created, a clone request must not change meaning if config is reloaded.

### Slug validation

Replace the GitHub-only validator with provider-aware validation:

- GitHub: exactly `owner/repo`.
- GitLab: at least two safe segments, allowing `group/subgroup/.../repo`.
- Apply the existing safe-character and directory checks to every segment.
- Reject empty segments, `.`, `..`, option-shaped segments, separators within a
  segment, control characters, and invalid provider-specific characters.
- Preserve credential redaction at rejection construction time.
- Preserve URL-clone validation unchanged.

The existing canonical repository identity already includes the host and supports
arbitrary path depth. Move/rename it only as part of the provider-neutral module
migration, update all imports directly, and retain tests for:

- GitHub HTTPS/SSH equivalence.
- GitLab HTTPS/SSH equivalence.
- Nested GitLab namespaces.
- Self-managed hostnames.
- Ports.
- Case normalization.
- Different hosts with identical paths remaining different.
- Local paths returning no hosted identity.

## GitHub adapter

Move existing GitHub behavior into the adapter without changing behavior.

Pin exact command shapes and existing parsers before adding GitLab behavior.

The compatibility `/github/repos` route must always call this adapter explicitly,
regardless of configured provider.

## GitLab adapter

### Availability

Use:

```sh
glab version
```

Distinguish:

- Spawn failure or non-runnable CLI: provider-aware unavailable error.
- Command succeeds: available.

### Noninteractive execution

For automated `glab` commands set:

```text
GLAB_NO_PROMPT=1
```

For hosted clone, also retain the existing Git/SSH noninteractive environment:

```text
GIT_TERMINAL_PROMPT=0
GIT_ASKPASS=
GIT_SSH_COMMAND=...BatchMode=yes...
```

When a GitLab hostname is present, set:

```text
GITLAB_HOST=<hostname>
```

Do not log repository sources that may contain credentials.

### Repository listing

Use this logical command shape, adjusting only if the recorded local `glab
--help` receipt requires it:

```sh
glab api projects \
  --method GET \
  --field membership=true \
  --field order_by=last_activity_at \
  --field sort=desc \
  --field per_page=100 \
  --paginate \
  --output ndjson \
  [--hostname <hostname>]
```

Use `membership=true`; do not use `glab repo list --all`.

Parse NDJSON as a stream of project objects.

Map:

| Normalized field | GitLab field |
|---|---|
| `full_name` | `path_with_namespace` |
| `namespace` | `namespace.full_path` |
| `name` | `path` |
| `description` | `description` |
| `visibility` | `visibility` |
| `fork` | `forked_from_project != null` |
| `archived` | `archived` |
| `default_branch` | nullable `default_branch` |
| `clone_url` | `http_url_to_repo` |
| `ssh_url` | `ssh_url_to_repo` |
| `activity_at` | nullable `last_activity_at` |

Use the existing bounded process-group runner and repository-list timeout.

### Hosted clone

Use:

```sh
glab repo clone <namespace/path> <destination>
```

Do not put `--` before the repository slug. In `glab`, `--` introduces Git
arguments after the repository/destination positions.

Revalidate the provider-specific slug immediately before constructing argv.

Apply `GLAB_NO_PROMPT`, `GITLAB_HOST` when present, and the existing Git/SSH
noninteractive environment.

Use the existing bounded clone runner and partial-destination cleanup.

### MR polling

Run from the project checkout:

```sh
glab api projects/:fullpath/merge_requests \
  --method GET \
  --field source_branch=<branch> \
  --field state=all \
  --field per_page=5 \
  --field order_by=created_at \
  --field sort=desc
```

`glab api` officially expands `:fullpath` using the current repository and
correctly handles nested namespace paths.

Normalize:

| Domain field | GitLab field |
|---|---|
| number | `iid` |
| URL | `web_url` |
| state | `opened`, `closed`, `merged` |
| draft | `draft`, falling back to `work_in_progress` |
| labels | label strings |
| target/base branch | `target_branch` |
| reviewers | `reviewers[].username` |
| created time | `created_at` |
| closed time | `closed_at` |
| merged time | `merged_at` |

Preserve:

- `Found` / `NotFound` / `FetchFailed`.
- Cached state on fetch failure.
- Open-MR preference when multiple results exist.
- The stale settled-branch defense based on ownership time.
- `u32` storage for GitLab IID, rejecting overflow instead of truncating.

Do not add approval API calls.

### Rich MR details

Run from the project checkout:

```sh
glab api projects/:fullpath/merge_requests/<iid> --method GET
```

Normalize:

- `title`
- `description` to the existing body
- `web_url`
- state
- draft/work-in-progress
- labels
- `head_pipeline.status`

The single-MR endpoint does not document `with_labels_details` as a supported
request parameter. For Phase 1, accept string labels and use an empty/default
color so existing UI fallback styling applies.

If label-color parity is added, use a separately documented endpoint or a
filtered list query with `with_labels_details=true`, and cover the extra call
with tests. Do not silently depend on an unsupported parameter.

### Stack retarget

Run from the project checkout:

```sh
glab mr update <iid> --target-branch <branch> --yes
```

Set `GLAB_NO_PROMPT=1`.

Preserve the existing difference between:

- Delete-time best-effort retargeting.
- User-initiated restacking, where failure must be reported.

### Agent stack instruction

Render:

```text
This branch is stacked on `<branch>` (not main).
When creating a merge request for this session, use:
glab mr create --target-branch <branch>
```

GitHub wording and command must remain unchanged when GitHub is selected.

## Runtime consistency

A live configuration implementation is preferred.

At the beginning of every logical operation, read and snapshot the provider once.
Pass it through all awaited steps. Do not reread it halfway through an operation.

This applies to:

- One entire PR/MR polling sweep.
- Repository listing.
- Clone request construction.
- Stack creation and prompt construction.
- User-initiated restacking.
- Delete-time child retargeting.
- Rich detail fetching.

The background poll loop should read the provider at the start of each tick.

Replace the single `gh_available` cache with provider-aware availability. A pair
of provider-keyed cells is sufficient. Repository-list actions should retain
their current fresh probe behavior so installing a CLI does not require
restarting.

When the provider changes:

- Clear or key the TUI enriched-review cache by provider.
- Clear or key “no enriched review found” suppression by provider.
- Refresh provider-aware status.
- Trigger a PR/MR refresh.
- Ensure no path remains on `gh` while another path has switched to `glab`.

Do not classify the provider as restart-required unless live switching proves
materially unsafe. If restart-required behavior is chosen instead, every
effectful path must continue using the startup provider until restart; partial
adoption is not acceptable.

## Errors

Replace `GhUnavailable` with a provider-aware missing-CLI error, for example:

```rust
CodeHostCliUnavailable {
    provider: CodeHostProvider,
}
```

Produce actionable messages:

- `GitHub CLI (gh) is not installed or not runnable`
- `GitLab CLI (glab) is not installed or not runnable`

Make listing timeout provider-neutral or provider-aware:

```rust
RepositoryListTimedOut {
    provider: CodeHostProvider,
    secs: u64,
}
```

Keep these categories distinct:

- Missing/unrunnable CLI.
- Authentication/network/API command failure.
- Malformed output.
- Repository-list timeout.
- Clone timeout.
- Rejected clone source.

Server and backend mappings:

- Missing CLI to 503 / unavailable.
- Invalid source or hostname to 400.
- Listing timeout to the existing timeout/unavailable transport category, with
  accurate text.
- Authentication/API failure must not be misreported as a missing CLI.

Background PR/MR polling continues to collapse provider errors into
`FetchFailed`, preserving cached state.

## Telemetry

The current telemetry feature method only accepts a static feature name and has
no arbitrary property API.

Use either:

```text
repository.list
```

or a fixed bounded pair:

```text
repository.list.github
repository.list.gitlab
```

Do not emit hostnames, repository names, paths, branches, or arbitrary provider
strings.

Do not redesign telemetry solely for this feature.

## Server and compatibility behavior

Add a provider-neutral endpoint:

```text
GET /repositories
```

It returns `RepositoryListing`.

Keep:

```text
GET /github/repos
```

with its existing GitHub-only payload and semantics.

Compatibility requirements:

### Old server and new client

- New client first calls `/repositories`.
- On 404 only, fall back to `/github/repos`.
- Treat fallback data as GitHub.
- Do not fall back on authentication, timeout, or 500 errors.

### New server and old client

- `/github/repos` always lists GitHub repositories through `gh`.
- `CloneSource::Github` always executes `gh`.
- The configured GitLab provider must not change the meaning of old requests.

### New server and new client

- `/repositories` returns the effective provider, hostname, and normalized
  repositories.
- Selecting a row creates an explicit GitHub or GitLab clone source.
- GitLab clone requests carry their hostname.

### Server status

Do not remove `ServerStatus.gh_available` during the compatibility release.

Keep it with its original meaning and add provider-aware status:

```rust
struct CodeHostStatus {
    provider: CodeHostProvider,
    hostname: Option<String>,
    cli_available: bool,
}
```

```rust
struct ServerStatus {
    gh_available: bool, // compatibility
    code_host: CodeHostStatus,
    tmux_ok: bool,
    version: String,
}
```

For an old server response, new clients default the provider to GitHub and derive
selected availability from `gh_available`.

Add `code_host_provider` and `gitlab_hostname` to the server config patch
allow-list with validation.

Update:

- Core backend trait.
- Local backend.
- Mock and placeholder backends.
- Backend error classification.
- Remote backend.
- Rust HTTP client.
- Server handlers/router/tests.
- Client timeout documentation and tests.

The client repository-list timeout must remain strictly longer than the server
subprocess budget.

## TUI

Use the actual `claude-commander-tui` crate paths.

Add “Code Host” under “Pull Requests & Sync” using the existing option picker:

- GitHub
- GitLab

Do not use free text or a `use_gitlab` boolean.

Generalize internal picker names:

- `GithubRepoPicker` to a provider-neutral equivalent.
- `GithubRepo` item to a hosted repository item.
- `GithubReposLoaded` to a repositories-loaded event.
- `refetch_github_repos` and related state to provider-neutral names.

Provider-aware text must cover:

- Clone action.
- Loading state.
- Empty state.
- Listing failure.
- Missing CLI guidance.
- Pull Request versus Merge Request.
- Retarget success/failure.
- Rich detail heading.
- Stack prompt/help text.

Keep the existing `pr_*` internal and persisted names where renaming provides no
functional benefit.

The local enriched-detail path currently bypasses the backend/service. Pass the
snapshotted provider to it or move the call behind a suitable service method
without violating the core/frontend boundary.

Changing provider must invalidate provider-sensitive TUI caches and request a
refresh.

## Flutter client

The server’s configured provider controls Flutter behavior. A general mobile
server-config editor is not required.

Update hand-written files covering:

- Clone repository page.
- Projects page.
- Commander API interface.
- Commander store.
- Rust bridge mirrors/simple API.
- Fake API and fixtures.
- Widget and server-flow tests.

The clone page must obtain provider and hostname from `RepositoryListing`,
including when the repository list is empty.

Update provider-aware labels and missing-CLI guidance.

Regenerate Flutter Rust Bridge bindings using the repository’s normal generator.
Never hand-edit generated files.

## Automated test matrix

### Provider/config

- Missing provider defaults to GitHub.
- GitHub and GitLab serialize/deserialize.
- Invalid provider gives a clear config error.
- Valid and invalid GitLab hostnames.
- Old config loads unchanged.
- TUI uses an option picker with exactly two values.
- Provider switching invalidates or rekeys all provider-sensitive caches.
- One polling sweep cannot switch providers halfway through.

### Wire compatibility

- Existing GitHub clone JSON round-trips unchanged.
- New GitLab clone JSON includes provider-specific hostname.
- Old repository payloads remain readable where required.
- Old server status defaults to GitHub in new clients.
- New server status retains `gh_available`.
- Empty `RepositoryListing` still communicates provider and hostname.

### Slug and identity safety

- GitHub accepts exactly two safe segments.
- GitHub rejects nested namespaces.
- GitLab accepts nested namespaces.
- GitLab requires at least namespace/project.
- Empty, `.`, `..`, option-shaped and control-character segments fail.
- Rejection messages redact credentials.
- GitLab SSH and HTTPS origins match the same picker row.
- Same path on GitHub and GitLab remains different.
- Self-managed host and ports are canonicalized correctly.
- URL cloning remains unchanged.

### Commands

For both CLIs, use stand-in executables/prepared commands:

- Availability success.
- Availability nonzero exit.
- Availability spawn failure.
- Exact GitHub and GitLab listing argv.
- Exact hosted-clone argv.
- GitHub retains its existing option terminator.
- GitLab does not put `--` before the slug.
- Exact retarget argv.
- Exact stack prompt.
- `GLAB_NO_PROMPT=1` on automated GitLab commands.
- GitLab clone receives the existing Git/SSH noninteractive environment.
- GitLab hostname is applied to listing and clone.
- GitHub behavior remains byte/argv compatible.
- Listing timeout kills child and descendants for both adapters.
- Missing CLI and timeout remain different errors.

### GitLab repository parsing

Fixtures must cover:

- Multiple NDJSON objects/pages.
- Nested group/subgroup/project.
- Public, internal and private visibility.
- Archived project.
- Fork.
- Empty/null default branch.
- Null activity timestamp.
- Empty output.
- Malformed JSON.
- Missing required identity fields.

### GitLab MR polling

Fixtures must cover:

- Open, closed, merged and locked/unknown state behavior.
- Draft and `work_in_progress` fallback.
- Labels.
- Target branch.
- Reviewers.
- Created/closed/merged timestamps.
- Multiple MRs for one source branch.
- Open MR preference.
- Stale settled-MR filtering.
- Empty result to `NotFound`.
- Command/API/malformed-output failure to `FetchFailed`.
- IID overflow rejection.

### Rich MR details

- Title, body, URL and state.
- Draft state.
- String labels with fallback color.
- No pipeline.
- Passing pipeline.
- Failing pipeline.
- Pending pipeline.
- Skipped pipeline.
- Canceled pipeline.
- Unknown non-empty pipeline status to pending.
- Malformed response returns no enriched result without corrupting cached data.

### Service/backend/transport

- Local provider-aware listing.
- Remote provider-aware listing.
- `/github/repos` remains GitHub-specific under GitLab config.
- New client falls back only on 404.
- Explicit GitLab clone hostname survives config changes.
- Missing `gh` and missing `glab` produce actionable 503 responses.
- URL clone works without either hosting CLI.
- Poll, rich detail, retarget and stack prompt all use the same selected provider.
- Client timeout remains longer than server listing budget.

### UI

- Default GitHub labels and PR wording remain unchanged.
- GitLab labels use GitLab/MR/glab wording.
- Empty listing still renders correct provider text.
- Nested namespaces group and filter correctly.
- Public/internal/private/archived badges are defined.
- Already-added matching works across GitLab SSH/HTTPS origins.
- URL clone remains usable when selected CLI is missing.
- Provider switch clears stale enriched details.
- Pipeline states render passing/failing/pending/none.

## Manual test checklist required in every PR

Every PR description must contain:

```markdown
## Owner manual test checklist

Environment:
- [ ] Claude Commander build/commit tested:
- [ ] `gh` version:
- [ ] `glab` version:
- [ ] GitHub host/account:
- [ ] GitLab host/account:
- [ ] Self-managed GitLab tested: yes / no / not applicable

Features introduced or changed in this PR:
- [ ] <specific feature and expected result>
- [ ] <specific feature and expected result>

Regression checks:
- [ ] Existing GitHub behavior still works
- [ ] URL clone still works
- [ ] Missing CLI behavior is actionable
- [ ] No repository credentials appear in errors or logs

Known limitations:
- <explicit limitations>
```

Do not write only “test GitLab support.” List each user-visible feature added by
that PR with expected behavior.

For the final GitLab feature PR, ask the owner to manually check:

### GitHub regression

- [ ] Default config still selects GitHub.
- [ ] GitHub repository picker lists repositories.
- [ ] Public and private GitHub repositories clone.
- [ ] PR badge polling still updates.
- [ ] Rich PR details and checks render.
- [ ] Creating a stacked session gives the `gh pr create --base` instruction.
- [ ] Restacking or deleting a middle stack session retargets the GitHub PR.

### GitLab

- [ ] Selecting GitLab changes picker and review wording.
- [ ] Repository picker lists personal/direct-membership projects.
- [ ] Nested group/subgroup projects appear correctly.
- [ ] Public, internal, private, archived and forked projects display correctly.
- [ ] A private GitLab project clones through authenticated `glab`.
- [ ] An empty project with no default branch does not break the picker.
- [ ] Existing GitLab SSH and HTTPS origins are recognized as already added.
- [ ] Open, closed, merged and draft MRs update session badges.
- [ ] Reviewers and target branches appear.
- [ ] Rich MR title/body/labels/pipeline state render.
- [ ] A stacked session gives the `glab mr create --target-branch` instruction.
- [ ] Restacking retargets the MR with `glab mr update`.
- [ ] Deleting a middle stack session retargets child MRs.
- [ ] Missing/unauthed `glab` produces useful guidance.
- [ ] Switching back to GitHub does not leave stale GitLab data visible.

### Self-managed GitLab

- [ ] Listing uses the configured hostname.
- [ ] Clone uses the configured hostname.
- [ ] MR operations inside a checkout use the repository’s authenticated host.
- [ ] Changing hostname after starting a clone does not redirect that clone.

### Provider limitation

- [ ] Confirm and document that only the globally selected provider receives
  PR/MR integration.

## Documentation

Update:

- `README.md`
- `docs/configuration.md`
- `docs/usage.md`
- TUI help text
- Flutter user-facing text
- Screenshot fixture stubs if provider behavior affects them
- Architecture guidance only where source-of-truth boundaries changed

Do not alter:

- GitHub Actions/release workflows.
- Homebrew/AUR source hosting.
- Cargo/Nix GitHub dependency URLs.
- Vendored Cargokit GitHub publishing support.
- Generic “GitHub-style diff” descriptions.
- Repository metadata unless the project itself is moving hosts.

Add `glab` to the development container only if contributors need it to exercise
this runtime feature. Keep `gh`.

## Recommended delivery sequence

Use one overarching implementation effort, with these review boundaries:

### PR 1: Provider-neutral internal refactor

- Extract GitHub adapter.
- Preserve current GitHub commands and behavior.
- Add normalized internal facade and command/parser tests.
- Do not expose a nonfunctional GitLab option.

### PR 2: Local GitLab core and TUI support

- Protocol/config provider model.
- GitLab hostname and validation.
- Repository listing and clone.
- MR polling, rich detail, retargeting and stack prompt.
- Runtime provider snapshotting and caches.
- TUI picker, settings and wording.
- Documentation of global-provider limitation.

### PR 3: Remote server and Flutter support

- `/repositories`.
- Compatibility `/github/repos`.
- Provider-aware status.
- Backend/remote/Rust client changes.
- Flutter API/store/pages/tests.
- Regenerated FRB bindings and affected goldens.
- Cross-version compatibility tests.

If implemented in one PR, preserve the same internal sequence in commits so
review remains tractable.

## Verification

After each implementation stage, run the narrowest relevant lane while
iterating.

Before declaring any PR ready:

```sh
scripts/verify.sh --all
```

If generated files, render snapshots, Flutter goldens, or other golden outputs
changed:

```sh
CC_FORCE_NIX=1 scripts/verify.sh --all
```

Report the real exit status. Do not claim verification is green if it was skipped
or blocked.

## Definition of done

- Unchanged config preserves all GitHub behavior.
- Selecting GitLab consistently routes all seven hosting operations through
  `glab`.
- Ordinary Git behavior remains unchanged.
- URL clone works without `gh` or `glab`.
- GitLab nested namespaces work.
- Self-managed listing and cloning work.
- Hosted clone requests freeze provider and hostname.
- Provider switching cannot split a single operation across providers.
- Missing CLI, auth/API failure, malformed output, listing timeout and clone
  timeout remain distinguishable.
- GitLab reviewers are retained without inventing review-decision semantics.
- Pipeline mapping is explicit and tested.
- Local, remote, TUI and Flutter behavior agree.
- Old/new client-server compatibility behaves as specified.
- Generated code and screenshots are regenerated through repository tooling.
- Every PR contains an owner-facing manual feature checklist.
- Repository-prescribed verification passes.
