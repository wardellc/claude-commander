import type { AgentState, AgentStatus } from "./generated/index.ts";
import { humanize } from "./state.ts";

export function needsAttention(state: AgentState): boolean {
  return state === "waiting_for_input" || state === "waiting_for_approval";
}

export function isFailure(state: AgentState): boolean {
  return state === "failed" || state === "rate_limited";
}

export function observationLabel(status: AgentStatus): string {
  const outcome = status.last_outcome ? ` · last turn ${humanize(status.last_outcome)}` : "";
  const date = new Date(Number(status.observed_at_ms));
  const observed = Number.isNaN(date.getTime()) ? "unknown" : date.toISOString();
  return `${humanize(status.state)}${outcome} · ${status.source} · observed ${observed}`;
}
