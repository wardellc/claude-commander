import '../src/rust/api/mirrors.dart';

extension AgentActivity on AgentState {
  bool get isBusy => switch (this) {
    AgentState.working ||
    AgentState.starting ||
    AgentState.compacting ||
    AgentState.retrying => true,
    _ => false,
  };

  bool get needsAttention =>
      this == AgentState.waitingForInput ||
      this == AgentState.waitingForApproval;
  bool get isFailure =>
      this == AgentState.failed || this == AgentState.rateLimited;

  String get statusLabel => switch (this) {
    AgentState.working => 'working',
    AgentState.idle => 'idle',
    AgentState.waitingForInput => 'waiting for input',
    AgentState.waitingForApproval => 'pending approval',
    AgentState.unknown => 'unknown',
    AgentState.starting => 'starting',
    AgentState.compacting => 'compacting',
    AgentState.retrying => 'retrying',
    AgentState.failed => 'failed',
    AgentState.rateLimited => 'usage limited',
    AgentState.interrupted => 'interrupted',
  };
}

String outcomeLabel(AgentTurnOutcome outcome) => switch (outcome) {
  AgentTurnOutcome.completed => 'completed',
  AgentTurnOutcome.failed => 'failed',
  AgentTurnOutcome.rateLimited => 'usage limited',
  AgentTurnOutcome.interrupted => 'interrupted',
};
