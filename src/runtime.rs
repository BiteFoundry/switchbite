//! Bounded, proposal-only blocking decisions. Dropping a caller never frees a running slot.
use serde_json::{Value, json};
use std::{fmt, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

/// Limits apply across every clone. Timeouts are measured from admission, including queue wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub max_concurrent: usize,
    pub max_queued: usize,
    pub queue_timeout_ms: u64,
    pub decision_timeout_ms: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            max_queued: 16,
            queue_timeout_ms: 5_000,
            decision_timeout_ms: 30_000,
        }
    }
}

/// Static diagnostics only: upstream messages and credentials are never incorporated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: &'static str,
    pub message: &'static str,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
}

impl RuntimeError {
    pub fn from_code(code: &str) -> Self {
        let (code, message, retryable, retry_after_ms) = match code {
            "local_judge_config" => (
                "local_judge_config",
                "Choose a pinned Laya model and a literal loopback HTTP endpoint; no cloud endpoint or key is used.",
                false,
                None,
            ),
            "local_judge_unavailable" => (
                "local_judge_unavailable",
                "Start the local Laya server, then retry this judgment. There is no cloud fallback.",
                true,
                Some(1000),
            ),
            "local_model_unavailable" => (
                "local_model_unavailable",
                "The local Laya model or endpoint is unavailable. Provision the selected model and check the local server.",
                false,
                None,
            ),
            "judge_context_too_long" => (
                "judge_context_too_long",
                "The local model cannot read all decision evidence. Shorten the task and candidate descriptions; no truncated judgment was accepted.",
                false,
                None,
            ),
            "invalid_request" => (
                "invalid_request",
                "Supply task id/objective and a pool with factual id, description, capabilities and availability. Check the tool schema for optional policy fields.",
                false,
                None,
            ),
            "invalid_judgment" => (
                "invalid_judgment",
                "The typed judgment could not be validated. Refresh it through the host.",
                false,
                None,
            ),
            "judge_failed" => (
                "judge_failed",
                "The decision judge failed. Ask the host to check the routing service.",
                false,
                None,
            ),
            "cancelled" => (
                "cancelled",
                "Routing was cancelled. Any running provider call may still settle; its result will not assign work.",
                false,
                None,
            ),
            "invalid_runtime_config" => (
                "invalid_runtime_config",
                "The decision limits are invalid.",
                false,
                None,
            ),
            "runtime_busy" => (
                "runtime_busy",
                "The decision queue is full. Try again shortly.",
                true,
                Some(1_000),
            ),
            "queue_timeout" => (
                "queue_timeout",
                "The decision waited too long to start.",
                true,
                Some(1_000),
            ),
            "decision_timeout" => (
                "decision_timeout",
                "The decision exceeded its time limit.",
                true,
                Some(1_000),
            ),
            "provider_auth_failed" => (
                "provider_auth_failed",
                "The decision provider could not authenticate.",
                false,
                None,
            ),
            "provider_rate_limited" => (
                "provider_rate_limited",
                "The decision provider is busy. Try again shortly.",
                true,
                Some(1_000),
            ),
            "provider_timeout" => (
                "provider_timeout",
                "The decision provider took too long to respond.",
                true,
                Some(1_000),
            ),
            "provider_unavailable" => (
                "provider_unavailable",
                "The decision provider is temporarily unavailable.",
                true,
                Some(1_000),
            ),
            "provider_invalid_response" => (
                "provider_invalid_response",
                "The decision provider returned an unusable response.",
                false,
                None,
            ),
            "unsafe_request" => (
                "unsafe_request",
                "The request could not be sent safely.",
                false,
                None,
            ),
            _ => (
                "runtime_failed",
                "The decision could not be completed.",
                false,
                None,
            ),
        };
        Self {
            code,
            message,
            retryable,
            retry_after_ms,
        }
    }

    pub fn to_value(&self) -> Value {
        // Re-canonicalize public fields so callers cannot accidentally echo upstream text.
        let safe = Self::from_code(self.code);
        let next_action = match safe.code {
            "local_judge_config" => "correct_configuration",
            "local_judge_unavailable" => "start_local_laya",
            "local_model_unavailable" => "provision_local_model",
            "judge_context_too_long" => "shorten_decision_context",
            "invalid_runtime_config" => "correct_configuration",
            "invalid_request" => "correct_input",
            "invalid_judgment" | "provider_invalid_response" => "refresh_judgment",
            "provider_auth_failed" => "configure_judge",
            "unsafe_request" => "remove_sensitive_data",
            "cancelled" => "stop",
            _ if safe.retryable => "retry_later",
            _ => "check_service",
        };
        json!({
            "code": safe.code, "message": safe.message, "retryable": safe.retryable,
            "retry_after_ms": if safe.retryable { self.retry_after_ms.map(|ms| ms.clamp(1, 30_000)).or(safe.retry_after_ms) } else { None },
            "next_action": next_action,
        })
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::from_code(self.code).message)
    }
}
impl std::error::Error for RuntimeError {}

#[derive(Clone)]
pub struct DecisionRuntime {
    config: RuntimeConfig,
    admission: Arc<Semaphore>,
    concurrent: Arc<Semaphore>,
    queued: Arc<Semaphore>,
}

// Aborting can stop a blocking job that has not started; running jobs retain their permits.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl DecisionRuntime {
    /// Caps: 1..=64 concurrent, 0..=1024 queued, 1..=300000 ms per timeout.
    pub fn new(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        if !(1..=64).contains(&config.max_concurrent)
            || config.max_queued > 1024
            || !(1..=300_000).contains(&config.queue_timeout_ms)
            || !(1..=300_000).contains(&config.decision_timeout_ms)
        {
            return Err(RuntimeError::from_code("invalid_runtime_config"));
        }
        Ok(Self {
            config,
            admission: Arc::new(Semaphore::new(config.max_concurrent + config.max_queued)),
            concurrent: Arc::new(Semaphore::new(config.max_concurrent)),
            queued: Arc::new(Semaphore::new(config.max_queued)),
        })
    }

    pub async fn execute<F>(&self, work: F) -> Result<Value, RuntimeError>
    where
        F: FnOnce() -> Result<Value, RuntimeError> + Send + 'static,
    {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(self.config.decision_timeout_ms);
        let admission = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| RuntimeError::from_code("runtime_busy"))?;
        let concurrent = match self.concurrent.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _queued = self
                    .queued
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| RuntimeError::from_code("runtime_busy"))?;
                let queue_deadline = started + Duration::from_millis(self.config.queue_timeout_ms);
                let permit = tokio::time::timeout_at(
                    queue_deadline.min(deadline),
                    self.concurrent.clone().acquire_owned(),
                )
                .await
                .map_err(|_| {
                    RuntimeError::from_code(if deadline <= queue_deadline {
                        "decision_timeout"
                    } else {
                        "queue_timeout"
                    })
                })?
                .map_err(|_| RuntimeError::from_code("runtime_failed"))?;
                if Instant::now() >= queue_deadline.min(deadline) {
                    return Err(RuntimeError::from_code(if deadline <= queue_deadline {
                        "decision_timeout"
                    } else {
                        "queue_timeout"
                    }));
                }
                permit
            }
        };
        if Instant::now() >= deadline {
            return Err(RuntimeError::from_code("decision_timeout"));
        }
        // No detached dispatcher or task per queued request. Only admitted work is spawned.
        let blocking_deadline = deadline.into_std();
        let mut job = AbortOnDrop(tokio::task::spawn_blocking(move || {
            let (_admission, _concurrent) = (admission, concurrent);
            if std::time::Instant::now() >= blocking_deadline {
                return Err(RuntimeError::from_code("decision_timeout"));
            }
            work()
        }));
        let result = tokio::time::timeout_at(deadline, &mut job.0)
            .await
            .map_err(|_| RuntimeError::from_code("decision_timeout"))?
            .map_err(|_| RuntimeError::from_code("runtime_failed"))?;
        if Instant::now() >= deadline {
            return Err(RuntimeError::from_code("decision_timeout"));
        }
        result
    }
}
