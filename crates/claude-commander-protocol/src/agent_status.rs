//! Agent activity observations. Current activity and the last turn's outcome
//! are independent: a new prompt must not erase the preceding failure.

use crate::session::AgentState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusSource {
    Terminal,
    Hook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AgentTurnOutcome {
    Completed,
    Failed,
    RateLimited,
    Interrupted,
}

impl std::fmt::Display for AgentTurnOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::RateLimited => "usage limited",
            Self::Interrupted => "interrupted",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentStatus {
    pub state: AgentState,
    pub source: AgentStatusSource,
    /// Unix milliseconds of the observation, not a claim that output is flowing.
    pub observed_at_ms: i64,
    pub last_outcome: Option<AgentTurnOutcome>,
    /// Monotonic within a pane generation; catches turns shorter than a poll.
    pub completed_turns: u32,
    /// Changes whenever the managed agent process is relaunched.
    pub generation: String,
}

impl AgentStatus {
    pub fn terminal(state: AgentState, observed_at_ms: i64) -> Self {
        Self {
            state,
            source: AgentStatusSource::Terminal,
            observed_at_ms,
            last_outcome: None,
            completed_turns: 0,
            generation: String::new(),
        }
    }

    /// Observation timestamps alone should not wake every connected client.
    pub fn same_activity(&self, other: &Self) -> bool {
        self.state == other.state
            && self.source == other.source
            && self.last_outcome == other.last_outcome
            && self.completed_turns == other.completed_turns
            && self.generation == other.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_comparison_ignores_only_observation_time() {
        let a = AgentStatus::terminal(AgentState::Unknown, 1);
        let mut b = a.clone();
        b.observed_at_ms = 2;
        assert!(a.same_activity(&b));
        b.last_outcome = Some(AgentTurnOutcome::Failed);
        assert!(!a.same_activity(&b));
    }
}
