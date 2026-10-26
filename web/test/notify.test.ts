import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { COMMANDER_SENTINEL_ID } from "../src/generated/constants.ts";
import type { AgentState, AgentStatus, SessionInfo } from "../src/generated/index.ts";
import { NotifyTracker } from "../src/notify.ts";

function session(id: string, unread = false): SessionInfo {
  return { id, session_id: id, title: id, unread } as SessionInfo;
}

function poll(sessions: SessionInfo[], agentStates: Record<string, AgentState> = {}) {
  return { sessions, agentStates };
}

test("failures and limits notify as failures even when unread also changes", () => {
  for (const state of ["failed", "rate_limited"] as const) {
    const t = new NotifyTracker();
    t.observe(poll([session("a")], { a: "working" }));
    assert.deepEqual(t.observe(poll([session("a", true)], { a: state })), [
      { key: "a", kind: "failed" },
    ]);
    assert.deepEqual(t.observe(poll([session("a", true)], { a: state })), []);
  }
});

test("approval and user questions both need attention without duplicate notifications", () => {
  const t = new NotifyTracker();
  t.observe(poll([session("a")], { a: "working" }));
  assert.deepEqual(t.observe(poll([session("a")], { a: "waiting_for_approval" })), [
    { key: "a", kind: "needs_input" },
  ]);
  assert.deepEqual(t.observe(poll([session("a")], { a: "waiting_for_input" })), []);
});

test("a short failed turn is not reported as finished after work resumes", () => {
  const t = new NotifyTracker();
  t.observe(poll([session("a")], { a: "working" }));
  assert.deepEqual(
    t.observe({
      ...poll([session("a", true)], { a: "working" }),
      observations: { a: { last_outcome: "failed" } as AgentStatus },
    }),
    [{ key: "a", kind: "failed" }],
  );
});

test("an unavailable inspection after interruption cannot notify successful completion", () => {
  const t = new NotifyTracker();
  t.observe(poll([session("a")], { a: "working" }));
  assert.deepEqual(
    t.observe({
      ...poll([session("a", true)], { a: "unknown" }),
      observations: { a: { last_outcome: "interrupted" } as AgentStatus },
    }),
    [],
  );
});

describe("NotifyTracker", () => {
  test("the first poll only baselines", () => {
    const t = new NotifyTracker();
    assert.deepEqual(t.observe(poll([session("a", true)], { a: "waiting_for_input" })), []);
  });

  test('"finished" fires when unread goes false → true', () => {
    const t = new NotifyTracker();
    // The poll never saw it working: the flag alone is the signal.
    t.observe(poll([session("a")], { a: "idle" }));
    assert.deepEqual(t.observe(poll([session("a", true)], { a: "idle" })), [
      { key: "a", kind: "finished" },
    ]);
    // Edge-triggered: still unread next poll is not a new event.
    assert.deepEqual(t.observe(poll([session("a", true)], { a: "idle" })), []);
  });

  test('a polled working → idle alone is not "finished"', () => {
    // The poll can miss or invent this edge (a 1.5s sample of a flapping
    // detector); `unread` is the server's own record that the agent finished.
    const t = new NotifyTracker();
    t.observe(poll([session("a")], { a: "working" }));
    assert.deepEqual(t.observe(poll([session("a")], { a: "idle" })), []);
  });

  test("needs-input fires on the transition into waiting_for_input", () => {
    const t = new NotifyTracker();
    t.observe(poll([session("a")], { a: "working" }));
    assert.deepEqual(t.observe(poll([session("a")], { a: "waiting_for_input" })), [
      { key: "a", kind: "needs_input" },
    ]);
    assert.deepEqual(t.observe(poll([session("a")], { a: "waiting_for_input" })), []);
  });

  test("never notifies for the commander sentinel", () => {
    const t = new NotifyTracker();
    t.observe(poll([], { [COMMANDER_SENTINEL_ID]: "working" }));
    assert.deepEqual(t.observe(poll([], { [COMMANDER_SENTINEL_ID]: "waiting_for_input" })), []);
    assert.deepEqual(t.observe(poll([], { [COMMANDER_SENTINEL_ID]: "idle" })), []);
  });

  test("a session first seen after the baseline is baselined, not announced", () => {
    const t = new NotifyTracker();
    t.observe(poll([]));
    assert.deepEqual(t.observe(poll([session("b", true)], { b: "waiting_for_input" })), []);
    assert.deepEqual(t.observe(poll([session("b", true)], { b: "idle" })), []);
  });
});
