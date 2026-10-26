//! Optional Claude/Codex lifecycle hooks, scoped to the launching tmux pane.
//! No prompt, command, result, or error text is retained.

use crate::session::AgentState;
use crate::tmux::TmuxExecutor;
use claude_commander_protocol::agent_status::{AgentStatus, AgentStatusSource, AgentTurnOutcome};
use serde_json::Value;

pub const STATUS_OPTION: &str = "@cc_agent_status";
pub const GENERATION_OPTION: &str = "@cc_agent_generation";
pub const GENERATION_ENV: &str = "CC_AGENT_GENERATION";
pub const MAX_HOOK_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    Starting,
    Working,
    Approval,
    Input,
    Compacting,
    Retrying,
    Completed,
    Failed,
    RateLimited,
    Interrupted,
    Ended,
}

/// Claude's hook reference: https://code.claude.com/docs/en/hooks
/// Codex's hook reference: https://developers.openai.com/codex/hooks
/// Only recognised lifecycle fields are read. Subagent events cannot settle
/// the parent turn. Unknown/new events leave the existing observation alone.
pub fn parse_hook(value: &Value) -> Option<HookEvent> {
    if value
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        || value
            .get("parent_tool_use_id")
            .is_some_and(|v| !v.is_null())
    {
        return None;
    }
    let name = value
        .get("hook_event_name")
        .or_else(|| value.get("hookEventName"))?
        .as_str()?;
    Some(match name {
        "SessionStart" => HookEvent::Starting,
        "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "PostCompact" => {
            HookEvent::Working
        }
        "PreToolUse"
            if value
                .get("tool_name")
                .or_else(|| value.get("toolName"))
                .and_then(Value::as_str)
                == Some("AskUserQuestion") =>
        {
            HookEvent::Input
        }
        "PreToolUse" => HookEvent::Working,
        "PermissionRequest" => HookEvent::Approval,
        "Notification" => match value.get("notification_type")?.as_str()? {
            "permission_prompt" => HookEvent::Approval,
            "elicitation_dialog" => HookEvent::Input,
            // An idle notification is not proof that a turn succeeded.
            _ => return None,
        },
        "PreCompact" => HookEvent::Compacting,
        "Stop" => HookEvent::Completed,
        "StopFailure" => match value.get("error").and_then(Value::as_str) {
            Some("rate_limit" | "rate_limit_error" | "billing_error") => HookEvent::RateLimited,
            _ => HookEvent::Failed,
        },
        "Interrupt" => HookEvent::Interrupted,
        "SessionEnd" => HookEvent::Ended,
        _ => return None,
    })
}

/// Reduce one event without erasing the previous outcome when new work starts.
pub fn apply_hook(
    previous: Option<&AgentStatus>,
    generation: &str,
    event: HookEvent,
    at: i64,
) -> AgentStatus {
    let mut status = previous
        .filter(|p| p.generation == generation)
        .cloned()
        .unwrap_or_else(|| AgentStatus::terminal(AgentState::Unknown, at));
    if status.observed_at_ms > at {
        return status;
    }
    status.generation = generation.to_owned();
    status.source = AgentStatusSource::Hook;
    status.observed_at_ms = at;
    status.state = match event {
        HookEvent::Starting => AgentState::Starting,
        HookEvent::Working => AgentState::Working,
        HookEvent::Approval => AgentState::WaitingForApproval,
        HookEvent::Input => AgentState::WaitingForInput,
        HookEvent::Compacting => AgentState::Compacting,
        HookEvent::Retrying => AgentState::Retrying,
        HookEvent::Completed => AgentState::Idle,
        HookEvent::Failed => AgentState::Failed,
        HookEvent::RateLimited => AgentState::RateLimited,
        HookEvent::Interrupted => AgentState::Interrupted,
        HookEvent::Ended => AgentState::Unknown,
    };
    let outcome = match event {
        HookEvent::Completed => Some(AgentTurnOutcome::Completed),
        HookEvent::Failed => Some(AgentTurnOutcome::Failed),
        HookEvent::RateLimited => Some(AgentTurnOutcome::RateLimited),
        HookEvent::Interrupted => Some(AgentTurnOutcome::Interrupted),
        _ => None,
    };
    if let Some(outcome) = outcome {
        status.last_outcome = Some(outcome);
        status.completed_turns = status.completed_turns.saturating_add(1);
    }
    status
}

/// Best-effort hook reporting. A single tmux command queue applies the
/// generation/time guard, counter update and observation without an external
/// lock that could be stranded when the harness times out a hook process.
pub async fn report_hook(
    executor: &TmuxExecutor,
    pane: &str,
    generation: &str,
    event: HookEvent,
) -> crate::Result<()> {
    report_hook_context(
        executor,
        pane,
        generation,
        event,
        HookIdentity {
            session_id: generation,
            turn_id: None,
            new_turn: false,
            reset_session: false,
        },
    )
    .await
}

/// Input IDs are correlation data only. Never keep the hook's free-form text.
/// Codex's generated hooks schemas include optional agent_id on turn hooks:
/// codex-rs/hooks/schema/generated/user-prompt-submit.command.input.schema.json
/// at 970111f6cb62cc02fe36a82ea7d71b37a2889c97.
pub async fn report_hook_payload(
    executor: &TmuxExecutor,
    pane: &str,
    generation: &str,
    value: &Value,
) -> crate::Result<()> {
    let Some(event) = parse_hook(value) else {
        return Ok(());
    };
    let Some(session_id) = value
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| safe_id(id))
    else {
        return Ok(());
    };
    let turn_id = value
        .get("turn_id")
        .and_then(Value::as_str)
        .filter(|id| safe_id(id));
    let new_turn = value
        .get("hook_event_name")
        .or_else(|| value.get("hookEventName"))
        .and_then(Value::as_str)
        == Some("UserPromptSubmit");
    let reset_session = event == HookEvent::Starting
        && matches!(
            value.get("source").and_then(Value::as_str),
            Some("clear" | "resume" | "compact")
        );
    report_hook_context(
        executor,
        pane,
        generation,
        event,
        HookIdentity {
            session_id,
            turn_id,
            new_turn,
            reset_session,
        },
    )
    .await
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b':'))
}

struct HookIdentity<'a> {
    session_id: &'a str,
    turn_id: Option<&'a str>,
    new_turn: bool,
    reset_session: bool,
}

async fn report_hook_context(
    executor: &TmuxExecutor,
    pane: &str,
    generation: &str,
    event: HookEvent,
    identity: HookIdentity<'_>,
) -> crate::Result<()> {
    let HookIdentity {
        session_id,
        turn_id,
        new_turn,
        reset_session,
    } = identity;
    if !pane.starts_with('%')
        || !pane[1..].chars().all(|c| c.is_ascii_digit())
        || pane.len() < 2
        || uuid::Uuid::parse_str(generation).is_err()
    {
        return Ok(());
    }
    let at = chrono::Utc::now().timestamp_millis();
    let candidate = apply_hook(None, generation, event, at);
    let finished = candidate.last_outcome.is_some();
    let mut json = serde_json::to_value(&candidate).expect("serializable status");
    json["completed_turns"] = Value::String("CC_COUNT".into());
    if !finished {
        json["last_outcome"] = Value::String("CC_OUTCOME".into());
    }
    let json = serde_json::to_string(&json)
        .expect("serializable status")
        .replace(
            "\"CC_COUNT\"",
            "#{?@cc_agent_completed_turns,#{@cc_agent_completed_turns},0}",
        )
        .replace(
            "\"CC_OUTCOME\"",
            "#{?@cc_agent_last_outcome,#{@cc_agent_last_outcome},null}",
        );
    let mut commands = Vec::new();
    commands.push(format!(
        "set-option -p -t {pane} @cc_agent_root_session '{session_id}'"
    ));
    if reset_session {
        commands.push(format!("set-option -p -t {pane} @cc_agent_turn ''"));
        commands.push(format!(
            "set-option -p -t {pane} @cc_agent_finished_turn ''"
        ));
        commands.push(format!(
            "set-option -p -t {pane} @cc_agent_last_outcome 'null'"
        ));
    }
    if let Some(turn) = turn_id {
        commands.push(format!("set-option -p -t {pane} @cc_agent_turn '{turn}'"));
        if finished {
            commands.push(format!(
                "set-option -p -t {pane} @cc_agent_finished_turn '{turn}'"
            ));
        }
    }
    if let Some(outcome) = candidate.last_outcome {
        let outcome = serde_json::to_string(&outcome).expect("enum");
        commands.push(format!("set-option -p -F -t {pane} @cc_agent_completed_turns '#{{e|+|:#{{?@cc_agent_completed_turns,#{{@cc_agent_completed_turns}},0}},1}}'"));
        commands.push(format!(
            "set-option -p -t {pane} @cc_agent_last_outcome '{outcome}'"
        ));
    }
    commands.push(format!(
        "set-option -p -t {pane} @cc_agent_observed_at '{at}'"
    ));
    commands.push(format!(
        "set-option -p -F -t {pane} {STATUS_OPTION} '{json}'"
    ));
    let mut guard = format!(
        "#{{&&:#{{==:#{{{GENERATION_OPTION}}},{generation}}},#{{>=:{at},#{{?@cc_agent_observed_at,#{{@cc_agent_observed_at}},0}}}}}}"
    );
    let root_guard = format!(
        "#{{||:#{{==:#{{@cc_agent_root_session}},}},#{{==:#{{@cc_agent_root_session}},{session_id}}}}}"
    );
    if !reset_session {
        guard = format!("#{{&&:{guard},{root_guard}}}");
    }
    if event == HookEvent::Starting && !reset_session {
        guard = format!("#{{&&:{guard},#{{==:#{{@cc_agent_root_session}},}}}}");
    }
    if reset_session {
        guard = format!("#{{&&:{guard},#{{!=:#{{@cc_agent_root_session}},{session_id}}}}}");
    }
    if let Some(turn) = turn_id
        && !new_turn
    {
        let turn_guard =
            format!("#{{||:#{{==:#{{@cc_agent_turn}},}},#{{==:#{{@cc_agent_turn}},{turn}}}}}");
        guard = format!("#{{&&:{guard},{turn_guard}}}");
        guard = format!("#{{&&:{guard},#{{!=:#{{@cc_agent_finished_turn}},{turn}}}}}");
    }
    executor
        .execute(&["if-shell", "-F", "-t", pane, &guard, &commands.join(" ; ")])
        .await?;
    Ok(())
}

/// A configuration fragment to merge into the harness's existing hook config.
/// Does not edit user settings or bypass the harness's hook trust policy.
pub fn hook_configuration(codex: bool) -> Value {
    let events = if codex {
        vec![
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
            "Stop",
            "Interrupt",
            "SessionEnd",
        ]
    } else {
        vec![
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PostToolUseFailure",
            "Notification",
            "PreCompact",
            "PostCompact",
            "Stop",
            "StopFailure",
            "SessionEnd",
        ]
    };
    let hooks: serde_json::Map<String, Value> = events.into_iter().map(|event| {
        (event.to_owned(), serde_json::json!([{"hooks":[{"type":"command", "command":"claude-commander agent-event", "timeout":3}]}]))
    }).collect();
    serde_json::json!({"hooks": hooks})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_hook_schemas_and_question_approval_are_distinct() {
        assert_eq!(
            parse_hook(&json!({"hook_event_name":"PermissionRequest"})),
            Some(HookEvent::Approval)
        );
        assert_eq!(
            parse_hook(&json!({"hookEventName":"PreToolUse", "toolName":"AskUserQuestion"})),
            Some(HookEvent::Input)
        );
        assert_eq!(
            parse_hook(
                &json!({"hook_event_name":"Notification", "notification_type":"idle_prompt"})
            ),
            None
        );
        assert_eq!(
            parse_hook(&json!({"hook_event_name":"Stop", "agent_id":"child"})),
            None
        );
    }

    #[test]
    fn turn_sequence_preserves_outcome_and_rejects_delayed_events() {
        let mut s = apply_hook(None, "a", HookEvent::Working, 1);
        for (event, state) in [
            (HookEvent::Approval, AgentState::WaitingForApproval),
            (HookEvent::Working, AgentState::Working),
            (HookEvent::Compacting, AgentState::Compacting),
            (HookEvent::Working, AgentState::Working),
            (HookEvent::Completed, AgentState::Idle),
        ] {
            s = apply_hook(Some(&s), "a", event, s.observed_at_ms + 1);
            assert_eq!(s.state, state);
        }
        assert_eq!(s.completed_turns, 1);
        s = apply_hook(Some(&s), "a", HookEvent::Working, 10);
        assert_eq!(s.last_outcome, Some(AgentTurnOutcome::Completed));
        assert_eq!(apply_hook(Some(&s), "a", HookEvent::Completed, 9), s);
        let restarted = apply_hook(Some(&s), "b", HookEvent::Starting, 11);
        assert_eq!(restarted.last_outcome, None);
        assert_eq!(restarted.completed_turns, 0);
    }

    #[test]
    fn failure_limit_and_interrupt_are_not_successes() {
        for (event, outcome) in [
            (HookEvent::Failed, AgentTurnOutcome::Failed),
            (HookEvent::RateLimited, AgentTurnOutcome::RateLimited),
            (HookEvent::Interrupted, AgentTurnOutcome::Interrupted),
        ] {
            let s = apply_hook(None, "a", event, 1);
            assert_eq!(s.last_outcome, Some(outcome));
            assert!(s.state.is_settled());
        }
    }
}
