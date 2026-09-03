//! Bounded agent-loop state. This crate plans and accounts; it never executes
//! host actions directly.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    pub max_steps: u32,
    pub token_budget: u64,
    pub tool_call_budget: u32,
    pub max_duration_ms: u64,
    pub loop_detection_window: usize,
    pub max_duplicate_actions: u32,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_steps: 32,
            token_budget: 32_000,
            tool_call_budget: 64,
            max_duration_ms: 15 * 60 * 1000,
            loop_detection_window: 16,
            max_duplicate_actions: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Thinking,
    WaitingForPermission,
    RunningTool,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AgentEvent {
    StateChanged { state: AgentState },
    StepStarted { step: u32 },
    ToolProposed { tool_id: String },
    LimitReached { limit: String },
    Cancelled,
}

pub struct AgentController {
    id: Uuid,
    limits: AgentLimits,
    state: AgentState,
    steps: u32,
    tokens_used: u64,
    tool_calls: u32,
    action_history: VecDeque<[u8; 32]>,
    started_at: Option<Instant>,
    cancellation: CancellationToken,
}

impl AgentController {
    pub fn new(limits: AgentLimits) -> Result<Self, AgentError> {
        if limits.max_steps == 0
            || limits.token_budget == 0
            || limits.tool_call_budget == 0
            || limits.max_duration_ms == 0
            || limits.loop_detection_window == 0
            || limits.max_duplicate_actions == 0
        {
            return Err(AgentError::InvalidLimits);
        }
        Ok(Self {
            id: Uuid::new_v4(),
            limits,
            state: AgentState::Idle,
            steps: 0,
            tokens_used: 0,
            tool_calls: 0,
            action_history: VecDeque::new(),
            started_at: None,
            cancellation: CancellationToken::new(),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    pub fn limits(&self) -> &AgentLimits {
        &self.limits
    }

    pub fn start(&mut self) {
        self.started_at = Some(Instant::now());
        self.state = AgentState::Thinking;
    }

    pub fn begin_step(&mut self) -> Result<u32, AgentError> {
        self.ensure_active()?;
        if self.steps >= self.limits.max_steps {
            self.state = AgentState::Failed;
            return Err(AgentError::LimitReached("max_steps"));
        }
        self.steps += 1;
        self.state = AgentState::Thinking;
        Ok(self.steps)
    }

    pub fn record_tokens(&mut self, tokens: u64) -> Result<(), AgentError> {
        self.ensure_active()?;
        self.tokens_used = self
            .tokens_used
            .checked_add(tokens)
            .ok_or(AgentError::LimitReached("token_budget"))?;
        if self.tokens_used > self.limits.token_budget {
            self.state = AgentState::Failed;
            return Err(AgentError::LimitReached("token_budget"));
        }
        Ok(())
    }

    pub fn register_tool_call(&mut self, fingerprint: &[u8]) -> Result<(), AgentError> {
        self.ensure_active()?;
        if self.tool_calls >= self.limits.tool_call_budget {
            self.state = AgentState::Failed;
            return Err(AgentError::LimitReached("tool_call_budget"));
        }
        let digest = *blake3::hash(fingerprint).as_bytes();
        let duplicate_count = self
            .action_history
            .iter()
            .filter(|candidate| **candidate == digest)
            .count() as u32;
        if duplicate_count >= self.limits.max_duplicate_actions {
            self.state = AgentState::Failed;
            return Err(AgentError::LoopDetected);
        }
        self.tool_calls += 1;
        self.action_history.push_back(digest);
        while self.action_history.len() > self.limits.loop_detection_window {
            self.action_history.pop_front();
        }
        self.state = AgentState::WaitingForPermission;
        Ok(())
    }

    pub fn mark_running_tool(&mut self) -> Result<(), AgentError> {
        self.ensure_active()?;
        self.state = AgentState::RunningTool;
        Ok(())
    }

    pub fn complete(&mut self) {
        if self.state != AgentState::Cancelled {
            self.state = AgentState::Completed;
        }
    }

    pub fn cancel(&mut self) {
        self.cancellation.cancel();
        self.state = AgentState::Cancelled;
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub fn steps(&self) -> u32 {
        self.steps
    }

    pub fn tokens_used(&self) -> u64 {
        self.tokens_used
    }

    pub fn tool_calls(&self) -> u32 {
        self.tool_calls
    }

    fn ensure_active(&mut self) -> Result<(), AgentError> {
        if self.cancellation.is_cancelled() {
            self.state = AgentState::Cancelled;
            return Err(AgentError::Cancelled);
        }
        if let Some(started_at) = self.started_at {
            if started_at.elapsed() > Duration::from_millis(self.limits.max_duration_ms) {
                self.state = AgentState::Failed;
                return Err(AgentError::LimitReached("max_duration_ms"));
            }
        } else {
            return Err(AgentError::NotStarted);
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent limits are invalid")]
    InvalidLimits,
    #[error("agent has not started")]
    NotStarted,
    #[error("agent was cancelled")]
    Cancelled,
    #[error("agent limit reached: {0}")]
    LimitReached(&'static str),
    #[error("repeated tool action detected")]
    LoopDetected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_budget_and_duplicate_guard_are_enforced() {
        let mut controller = AgentController::new(AgentLimits {
            max_duplicate_actions: 1,
            ..AgentLimits::default()
        })
        .expect("valid limits");
        controller.start();
        controller.begin_step().expect("step");
        controller.register_tool_call(b"read:README").expect("first call");
        assert!(matches!(
            controller.register_tool_call(b"read:README"),
            Err(AgentError::LoopDetected)
        ));
    }

    #[test]
    fn cancellation_is_visible_to_controller() {
        let mut controller = AgentController::new(AgentLimits::default()).expect("valid limits");
        controller.start();
        controller.cancel();
        assert!(controller.is_cancelled());
        assert!(matches!(controller.begin_step(), Err(AgentError::Cancelled)));
    }
}

