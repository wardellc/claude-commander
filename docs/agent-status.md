# Claude and Codex status

Commander reports current activity separately from the last observed turn
outcome. The TUI Info tab, browser information panel, and Flutter session detail
show the outcome where available. The agent-state API also includes observation
source, time, launch generation, and a completed-turn counter.

Update clients alongside the server: older clients do not understand the new
activity values. Updated clients can still read older servers' snapshots without
the optional observation data.

Activity includes working, starting, pending approval, waiting for input,
compacting, retrying, failed, usage limited, interrupted, idle, and unknown.
Approval and input requests both contribute to attention filters and workspace
counts. Failures take priority over unread completion markers.

## Terminal detection

Existing sessions work without additional configuration. Commander inspects the
active tmux pane, including process exit status, title and visible content.
Permission prompts take priority over a stale working title. An empty pane or
unrecognised output is **unknown**, rather than proof that a turn completed.
Submitted prompt text is not treated as an idle input prompt.

Unknown, starting, busy, compacting, retrying, and attention states are protected
from idle hibernation. Only positively settled states are eligible, subject to
the existing timeout, attachment, and keep-alive rules.

Terminal parsing is a fallback. It cannot reliably distinguish every API error
from a successful response, and polling can miss short turns. Unfamiliar UI
wording may produce unknown status. Use lifecycle hooks for stronger signals.

## Optional lifecycle hooks

The `claude-commander` executable must be on the agent's PATH. Print the fragment
for the harness you use:

```sh
claude-commander agent-hooks claude
claude-commander agent-hooks codex
```

Merge the printed `hooks` entries with your existing configuration:

- Claude: your Claude settings JSON, or a separate settings file passed with
  `claude --settings /absolute/path/to/commander-hooks.json`.
- Codex: `$CODEX_HOME/hooks.json` (normally `~/.codex/hooks.json`). Approve the
  added hooks through Codex's normal trust flow. Hooks can be disabled by your
  version or managed policy; Commander does not bypass those settings.

Do not replace existing hooks. The command prints configuration and never edits
your settings. You can use the Claude settings-file form in Commander's session
program setting, for example `claude --settings /absolute/path/to/commander-hooks.json`.

Restart managed sessions after upgrading Commander so the launch generation is
installed. The hook handler is inert outside managed panes. Normal shell and
paired-shell sessions remain unaffected.

Hook reports keep only fixed status values and correlation identifiers; prompts,
tool arguments, responses, transcript paths, and error messages are not stored
or sent to clients. Every report is guarded by its pane's launch generation.
Root-session binding excludes unrelated sessions; explicit subagent identifiers
are ignored. Where Codex supplies turn IDs, stale completion reports and
duplicate terminal events are rejected. Claude hooks do not provide equivalent
turn IDs, so correlation there is limited to root session, launch generation,
and report ordering.

Completed-turn counters preserve events between polls. The first observation
sets a baseline and never creates a historical unread notification. New work
retains the preceding outcome until another terminal event arrives.

If an attention hook is left behind after cancelling a dialog, positive
terminal evidence can recover after a short grace period. A missing completion
hook with an idle-looking pane becomes unknown after a grace period, without
inventing a successful result or allowing hibernation. A provider hook's `Stop`
reports a stop attempt; other custom Stop hooks may continue that turn, so it
does not provide the SDK's exact final-result semantics.

Hook availability and payloads vary by harness version. References:
[Claude hooks](https://code.claude.com/docs/en/hooks),
[Codex hooks](https://developers.openai.com/codex/hooks).
