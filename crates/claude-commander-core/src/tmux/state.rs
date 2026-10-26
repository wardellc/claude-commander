//! Shared agent observations from tmux panes and optional lifecycle hooks.
use super::TmuxExecutor;
use crate::agent::AgentKind;
use crate::session::{AgentState, SessionId};
use claude_commander_protocol::agent_status::{AgentStatus, AgentStatusSource, AgentTurnOutcome};
use futures::StreamExt;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};
use tracing::debug;

const PANE_FORMAT: &str = "#{session_name}\t#{window_active}\t#{pane_active}\t#{pane_dead}\t#{pane_dead_status}\t#{@cc_agent_generation}\t#{@cc_agent_status}\t#{pane_title}";

#[derive(Clone, Debug)]
struct PaneInfo {
    title: String,
    generation: String,
    hook: Option<AgentStatus>,
    dead: bool,
    exit_code: Option<i32>,
}

fn pane_infos(output: &str) -> HashMap<String, PaneInfo> {
    output
        .lines()
        .filter_map(|line| {
            let mut f = line.splitn(8, '\t');
            let name = f.next()?;
            if f.next()? != "1" || f.next()? != "1" {
                return None;
            }
            let dead = f.next()? == "1";
            let exit_code = f.next()?.parse().ok();
            let generation = f.next()?.to_string();
            let raw = f.next()?;
            let hook = (raw.len() <= 4096)
                .then(|| serde_json::from_str::<AgentStatus>(raw).ok())
                .flatten()
                .filter(|s| {
                    !generation.is_empty()
                        && s.generation == generation
                        && s.source == AgentStatusSource::Hook
                        && s.observed_at_ms <= chrono::Utc::now().timestamp_millis() + 5000
                });
            Some((
                name.to_string(),
                PaneInfo {
                    title: f.next()?.to_string(),
                    generation,
                    hook,
                    dead,
                    exit_code,
                },
            ))
        })
        .collect()
}

fn resolve_observation(
    kind: AgentKind,
    pane: &PaneInfo,
    content: &str,
    previous: Option<&AgentStatus>,
    now: i64,
) -> AgentStatus {
    let content_state = kind.content_state(content);
    let terminal = if content_state.needs_attention() || content_state.is_busy() {
        content_state
    } else {
        kind.title_state(&pane.title).unwrap_or(content_state)
    };
    let mut status = pane
        .hook
        .clone()
        .unwrap_or_else(|| AgentStatus::terminal(terminal, now));
    if pane.hook.is_none() {
        status.generation.clone_from(&pane.generation);
        if let Some(p) = previous.filter(|p| p.generation == pane.generation) {
            status.last_outcome = p.last_outcome;
            status.completed_turns = p.completed_turns;
            if terminal == AgentState::Idle && p.state.is_busy() {
                status.last_outcome = Some(AgentTurnOutcome::Completed);
                status.completed_turns = status.completed_turns.saturating_add(1);
            }
        }
    } else if !status.state.needs_attention()
        && (terminal.needs_attention()
            || (status.state.is_settled() && terminal.is_busy())
            || (status.state == AgentState::Starting && terminal == AgentState::Idle))
    {
        status.state = terminal;
        status.source = AgentStatusSource::Terminal;
        status.observed_at_ms = now;
    } else if status.state.needs_attention()
        && !terminal.needs_attention()
        && terminal != AgentState::Unknown
        && now.saturating_sub(status.observed_at_ms) > 5_000
    {
        status.state = if terminal == AgentState::Idle {
            AgentState::Unknown
        } else {
            terminal
        };
        status.source = AgentStatusSource::Terminal;
        status.observed_at_ms = now;
    } else if status.state.is_busy()
        && terminal == AgentState::Idle
        && now.saturating_sub(status.observed_at_ms) > 15_000
    {
        // Missing Stop is not success. A stale active hook with a positive
        // idle-looking screen becomes Unknown, preserving hibernation safety.
        status.state = AgentState::Unknown;
        status.source = AgentStatusSource::Terminal;
        status.observed_at_ms = now;
    } else if status.state == AgentState::Starting
        && now.saturating_sub(status.observed_at_ms) > 30_000
    {
        status.state = AgentState::Unknown;
        status.source = AgentStatusSource::Terminal;
        status.observed_at_ms = now;
    }
    if pane.dead {
        status.state = if pane.exit_code.is_some_and(|c| c != 0) {
            AgentState::Failed
        } else {
            AgentState::Unknown
        };
        status.source = AgentStatusSource::Terminal;
        status.observed_at_ms = now;
        if status.state == AgentState::Failed {
            status.last_outcome = Some(AgentTurnOutcome::Failed);
        }
    }
    status
}

fn unavailable(previous: Option<&AgentStatus>, now: i64) -> AgentStatus {
    let mut status = previous
        .cloned()
        .unwrap_or_else(|| AgentStatus::terminal(AgentState::Unknown, now));
    status.state = AgentState::Unknown;
    status.source = AgentStatusSource::Terminal;
    status.observed_at_ms = now;
    status
}

pub struct AgentStateDetector {
    executor: TmuxExecutor,
    cache: HashMap<String, (AgentState, Instant)>,
    cache_ttl: Duration,
    observations: BTreeMap<SessionId, AgentStatus>,
}
impl AgentStateDetector {
    pub fn new(executor: TmuxExecutor, cache_ttl: Duration) -> Self {
        Self {
            executor,
            cache: HashMap::new(),
            cache_ttl,
            observations: BTreeMap::new(),
        }
    }
    pub fn observations(&self) -> BTreeMap<SessionId, AgentStatus> {
        self.observations.clone()
    }

    pub fn seed_observations(&mut self, observations: BTreeMap<SessionId, AgentStatus>) {
        self.observations = observations;
    }

    pub async fn detect(&mut self, kind: AgentKind, name: &str) -> AgentState {
        if kind == AgentKind::Unknown {
            return AgentState::Unknown;
        }
        if let Some((state, at)) = self.cache.get(name)
            && at.elapsed() < self.cache_ttl
        {
            return *state;
        }
        let panes = self
            .executor
            .execute(&["list-panes", "-t", name, "-F", PANE_FORMAT])
            .await;
        let pane = panes.ok().and_then(|s| pane_infos(&s).remove(name));
        let state = if let Some(pane) = pane {
            match self.capture_visible_pane(name).await {
                Ok(content) => {
                    resolve_observation(
                        kind,
                        &pane,
                        &content,
                        None,
                        chrono::Utc::now().timestamp_millis(),
                    )
                    .state
                }
                Err(_) => AgentState::Unknown,
            }
        } else {
            AgentState::Unknown
        };
        self.cache.insert(name.to_string(), (state, Instant::now()));
        state
    }
    async fn capture_visible_pane(&self, name: &str) -> crate::Result<String> {
        self.executor
            .execute(&["capture-pane", "-t", name, "-p"])
            .await
    }
    pub async fn detect_all(
        &mut self,
        sessions: &[(SessionId, String, String)],
    ) -> BTreeMap<SessionId, AgentState> {
        self.observations
            .retain(|id, _| sessions.iter().any(|(sid, _, _)| sid == id));
        self.cache
            .retain(|name, _| sessions.iter().any(|(_, n, _)| n == name));
        let mut results = BTreeMap::new();
        let mut pending = Vec::new();
        for (id, name, program) in sessions {
            let kind = AgentKind::from_program(program);
            if kind == AgentKind::Unknown {
                results.insert(*id, AgentState::Unknown);
                self.observations.insert(
                    *id,
                    AgentStatus::terminal(
                        AgentState::Unknown,
                        chrono::Utc::now().timestamp_millis(),
                    ),
                );
            } else if let Some((state, at)) = self.cache.get(name)
                && at.elapsed() < self.cache_ttl
                && self.observations.contains_key(id)
            {
                results.insert(*id, *state);
            } else {
                pending.push((*id, name.clone(), kind));
            }
        }
        if pending.is_empty() {
            return results;
        }
        let panes = self
            .executor
            .execute(&["list-panes", "-a", "-F", PANE_FORMAT])
            .await
            .map(|s| pane_infos(&s))
            .unwrap_or_default();
        let detector = &*self;
        let panes = &panes;
        let detected: Vec<_> =
            futures::stream::iter(pending.into_iter().map(|(id, name, kind)| async move {
                let now = chrono::Utc::now().timestamp_millis();
                let status = if let Some(pane) = panes.get(&name) {
                    match detector.capture_visible_pane(&name).await {
                        Ok(content) => resolve_observation(
                            kind,
                            pane,
                            &content,
                            detector.observations.get(&id),
                            now,
                        ),
                        Err(e) => {
                            debug!("Failed to inspect agent pane {name}: {e}");
                            unavailable(
                                pane.hook
                                    .as_ref()
                                    .or_else(|| detector.observations.get(&id)),
                                now,
                            )
                        }
                    }
                } else {
                    unavailable(detector.observations.get(&id), now)
                };
                (id, name, status)
            }))
            .buffer_unordered(8)
            .collect()
            .await;
        for (id, name, status) in detected {
            self.cache.insert(name, (status.state, Instant::now()));
            results.insert(id, status.state);
            self.observations.insert(id, status);
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(hook: Option<AgentStatus>) -> PaneInfo {
        PaneInfo {
            title: "◐ busy".into(),
            generation: "launch".into(),
            hook,
            dead: false,
            exit_code: None,
        }
    }

    #[test]
    fn approval_beats_a_stale_working_title() {
        let status = resolve_observation(AgentKind::Claude, &pane(None), "Esc to cancel", None, 1);
        assert_eq!(status.state, AgentState::WaitingForApproval);
    }

    #[test]
    fn crash_is_not_success_even_with_a_stale_prompt() {
        let mut p = pane(None);
        p.dead = true;
        p.exit_code = Some(1);
        let status = resolve_observation(AgentKind::Claude, &p, "❯", None, 1);
        assert_eq!(status.state, AgentState::Failed);
        assert_eq!(status.last_outcome, Some(AgentTurnOutcome::Failed));
    }

    #[test]
    fn hook_failure_survives_idle_but_new_work_keeps_last_outcome() {
        let mut h = crate::agent_status::apply_hook(
            None,
            "launch",
            crate::agent_status::HookEvent::Failed,
            1,
        );
        let mut p = pane(Some(h.clone()));
        p.title.clear();
        let status = resolve_observation(AgentKind::Claude, &p, "❯", None, 2);
        assert_eq!(status.state, AgentState::Failed);
        p.title = "◐ busy".into();
        h = resolve_observation(AgentKind::Claude, &p, "❯", None, 3);
        assert_eq!(h.state, AgentState::Working);
        assert_eq!(h.last_outcome, Some(AgentTurnOutcome::Failed));
    }

    #[test]
    fn pane_metadata_rejects_old_launches_and_malformed_hook_records() {
        let h = crate::agent_status::apply_hook(
            None,
            "old",
            crate::agent_status::HookEvent::Completed,
            1,
        );
        let raw = serde_json::to_string(&h).unwrap();
        let p = pane_infos(&format!("s\t1\t1\t0\t\tnew\t{raw}\t✳ idle"));
        assert!(p["s"].hook.is_none());
        assert!(
            pane_infos("s\t1\t1\t0\t\tnew\tbroken\ttitle")["s"]
                .hook
                .is_none()
        );
    }

    #[test]
    fn unknown_never_creates_a_completion() {
        let mut p = pane(None);
        p.title.clear();
        let mut previous = AgentStatus::terminal(AgentState::Working, 1);
        previous.generation = "launch".into();
        let status = resolve_observation(AgentKind::Claude, &p, "", Some(&previous), 2);
        assert_eq!(status.state, AgentState::Unknown);
        assert_eq!(status.completed_turns, 0);
        assert_eq!(status.last_outcome, None);
    }

    #[test]
    fn cancelled_approval_recovers_from_positive_terminal_evidence() {
        let h = crate::agent_status::apply_hook(
            None,
            "launch",
            crate::agent_status::HookEvent::Approval,
            1,
        );
        let p = pane(Some(h));
        let resumed = resolve_observation(AgentKind::Claude, &p, "❯", None, 6_000);
        assert_eq!(resumed.state, AgentState::Working);
        let mut p = p;
        p.title.clear();
        assert_eq!(
            resolve_observation(AgentKind::Claude, &p, "❯", Some(&resumed), 7_000).state,
            AgentState::Unknown
        );
    }

    #[test]
    fn dropped_completion_hook_is_unknown_rather_than_success() {
        let h = crate::agent_status::apply_hook(
            None,
            "launch",
            crate::agent_status::HookEvent::Working,
            1,
        );
        let mut p = pane(Some(h));
        p.title.clear();
        let status = resolve_observation(AgentKind::Claude, &p, "❯", None, 16_000);
        assert_eq!(status.state, AgentState::Unknown);
        assert_eq!(status.last_outcome, None);
    }
    #[test]
    fn batch_titles_select_only_the_active_window_and_pane() {
        let titles = pane_infos(
            "s\t0\t1\t0\t\t\t\tinactive window\ns\t1\t0\t0\t\t\t\tinactive pane\ns\t1\t1\t0\t\t\t\tworking\twith tab\nmalformed\n",
        );
        assert_eq!(titles.len(), 1);
        assert_eq!(titles["s"].title, "working\twith tab");
    }

    // -- cache TTL boundary tests --
    //
    // These target the cache freshness check at the top of `detect()`:
    //   if let Some((state, cached_at)) = self.cache.get(tmux_session_name)
    //       && cached_at.elapsed() < self.cache_ttl
    //   { return *state; }
    //
    // They distinguish `<` from `<=`/`==`/`>` by pre-populating the private
    // `cache` field directly (same-module privacy) so we can place entries at
    // precise points relative to the TTL window without wall-clock sleeps.
    // `Instant::elapsed()` saturates to `Duration::ZERO` for instants in the
    // future, giving us an exact boundary handle.

    #[tokio::test]
    async fn test_detect_cache_hit_with_fresh_entry() {
        // Large TTL + just-now cached_at => elapsed is tiny relative to TTL, so
        // the freshness check is firmly inside the cache window. Returns the
        // cached state without invoking the executor.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(3600));
        detector.cache.insert(
            "tts-fresh".to_string(),
            (AgentState::Working, Instant::now()),
        );

        let result = detector.detect(AgentKind::Claude, "tts-fresh").await;
        assert_eq!(
            result,
            AgentState::Working,
            "fresh cache entry within TTL must hit and return the cached state"
        );
    }

    #[tokio::test]
    async fn test_detect_cache_miss_at_exact_boundary() {
        // Future cached_at + a TTL of zero yields `elapsed == ttl == 0`
        // deterministically. Correct `<` says miss; `<=`/`==` say hit. We
        // observe hit-vs-miss by inspecting the cache entry's instant after the
        // call: on a miss `detect_fresh` re-inserts a now-stamped entry whose
        // `elapsed()` becomes positive immediately.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::ZERO);
        let future = Instant::now() + Duration::from_secs(3600);
        detector
            .cache
            .insert("tts-boundary".to_string(), (AgentState::Working, future));

        let _ = detector.detect(AgentKind::Claude, "tts-boundary").await;

        let (_, after) = detector
            .cache
            .get("tts-boundary")
            .expect("cache entry must still exist after detect");
        assert!(
            after.elapsed() > Duration::ZERO,
            "with ttl=0 and elapsed=0, correct `<` must miss and re-insert a \
             now-stamped entry; a `<=`/`==` mutant would have hit and left the \
             original future instant in place (elapsed == 0)"
        );
    }

    // -- unknown-kind short-circuit --

    #[tokio::test]
    async fn test_detect_unknown_kind_short_circuits_before_cache() {
        // An unrecognised harness must report Unknown (treated as active by the
        // hibernation loop), NOT fall through to the content heuristic's Idle.
        // Proven by seeding the cache with Idle: the Unknown-kind guard runs
        // before the cache check, so it must win and return Unknown. Without the
        // guard, `detect` would hit the fresh cache entry and return Idle —
        // which is exactly the read that would get an active session hibernated.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(3600));
        detector
            .cache
            .insert("uk-sess".to_string(), (AgentState::Idle, Instant::now()));

        let state = detector.detect(AgentKind::Unknown, "uk-sess").await;
        assert_eq!(state, AgentState::Unknown);
    }

    // -- detect_all filtering --

    #[tokio::test]
    async fn test_detect_all_marks_unknown_program_unknown() {
        // Non-agent programs get Unknown without attempting tmux inspection.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(10));

        let sessions = vec![(
            SessionId::new(),
            "non-existent-session".to_string(),
            "bash".to_string(),
        )];

        let results = detector.detect_all(&sessions).await;
        assert_eq!(results.len(), 1);
        for state in results.values() {
            assert_eq!(*state, AgentState::Unknown);
        }
    }
}
