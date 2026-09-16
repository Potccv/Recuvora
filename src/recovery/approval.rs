//! Trusted, durable approval decisions, separate from simulation authorization.
//!
//! Callers are trusted host code: this is not an authentication boundary against
//! arbitrary code in the host process. Model output is evidence, never a permit.

mod paths;
mod storage;
mod transitions;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs::File, sync::Arc};
use thiserror::Error;

const MAX_ID: usize = 512;
const MAX_REASON: usize = 8192;
const MAX_ACTION: usize = 131_072;
const MAX_RECORD: usize = 262_144;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewerConfig {
    Human,
    Harness { harness_id: String },
}

/// Loaded by the host from trusted configuration, outside AI-writable targets.
/// Target and action-kind lists are exact matches and hard limits for all reviewers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalPolicy {
    pub id: String,
    pub version: u64,
    pub reviewer: ReviewerConfig,
    pub delegation: String,
    pub allowed_targets: Vec<String>,
    pub allowed_action_kinds: Vec<String>,
    pub ttl_secs: u64,
}

impl ApprovalPolicy {
    pub fn validate(&self) -> Result<(), ApprovalError> {
        text(&self.id, MAX_ID)?;
        text(&self.delegation, MAX_REASON)?;
        if self.version == 0 || !(1..=86_400).contains(&self.ttl_secs) {
            return Err(ApprovalError::Invalid("policy version or TTL"));
        }
        if let ReviewerConfig::Harness { harness_id } = &self.reviewer {
            text(harness_id, MAX_ID)?;
        }
        for values in [&self.allowed_targets, &self.allowed_action_kinds] {
            if values.is_empty() || values.len() > 128 {
                return Err(ApprovalError::Invalid(
                    "policy scope must contain 1..128 entries",
                ));
            }
            for value in values {
                text(value, MAX_ID)?;
            }
        }
        Ok(())
    }

    pub fn allows(&self, operation: &ProposedOperation) -> bool {
        self.allowed_targets.contains(&operation.target)
            && operation
                .action
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| {
                    self.allowed_action_kinds
                        .iter()
                        .any(|allowed| allowed == kind)
                })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedOperation {
    pub task_id: String,
    pub task_revision: u64,
    pub operation_id: String,
    pub target: String,
    pub action: Value,
}

impl ProposedOperation {
    pub fn validate(&self) -> Result<(), ApprovalError> {
        for value in [&self.task_id, &self.operation_id, &self.target] {
            text(value, MAX_ID)?;
        }
        let kind =
            self.action
                .get("kind")
                .and_then(Value::as_str)
                .ok_or(ApprovalError::Invalid(
                    "action must be an object with a string kind",
                ))?;
        text(kind, MAX_ID)?;
        if serde_json::to_vec(&self.action)?.len() > MAX_ACTION {
            return Err(ApprovalError::Invalid("action is too large"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
    Escalate,
}

/// This is the entire model response. Reviewer identity is supplied by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAssessment {
    pub request_id: String,
    pub decision: ApprovalDecision,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerIdentity {
    pub harness_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssessmentSource {
    Harness {
        harness_id: String,
        session_id: String,
    },
    Human {
        actor: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalAssessment {
    pub decision: ApprovalDecision,
    pub reason: String,
    pub reviewer: AssessmentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    WaitingHuman,
    Approved,
    Denied,
    Revoked,
    Canceled,
    Expired,
    Executing,
    Executed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub operation: ProposedOperation,
    pub policy: ApprovalPolicy,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRecord {
    pub request: ApprovalRequest,
    pub state: ApprovalState,
    pub revision: u64,
    pub updated_at: u64,
    pub assessment: Option<ApprovalAssessment>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Executed,
    Failed,
    Unknown,
}

/// A one-use, non-cloneable capability produced only after the execution intent
/// was synced. It does not implement Deserialize and its fields are private.
#[derive(Debug)]
pub struct ExecutionPermit {
    request_id: String,
    revision: u64,
    operation: ProposedOperation,
    store_identity: Arc<()>,
}

impl ExecutionPermit {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
    pub fn operation(&self) -> &ProposedOperation {
        &self.operation
    }
}

#[derive(Debug, Clone)]
pub struct ApprovalStoreConfig {
    pub max_requests: usize,
    pub max_journal_bytes: u64,
}

impl Default for ApprovalStoreConfig {
    fn default() -> Self {
        Self {
            max_requests: 10_000,
            max_journal_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("invalid approval input: {0}")]
    Invalid(&'static str),
    #[error("approval request was not found")]
    NotFound,
    #[error("approval input conflicts with the durable request")]
    Conflict,
    #[error("operation falls outside the trusted policy scope")]
    OutOfScope,
    #[error("approval is expired")]
    Expired,
    #[error("approval transition is not allowed from {0:?}")]
    InvalidState(ApprovalState),
    #[error("target has executing or unresolved work")]
    TargetBusy,
    #[error("approval storage capacity reached")]
    Capacity,
    #[error("approval journal writer is already locked: {0}")]
    Locked(std::io::Error),
    #[error("approval journal is corrupt: {0}")]
    Corrupt(String),
    #[error("approval store is unavailable after an I/O failure")]
    Unavailable,
    #[error("approval storage I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("approval JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalEntry {
    format: u32,
    sequence: u64,
    now: u64,
    event: Event,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Requested { request: ApprovalRequest },
    Changed { request_id: String, change: Change },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Assess {
        assessment: ApprovalAssessment,
    },
    HumanDecision {
        assessment: ApprovalAssessment,
    },
    WaitingHuman {
        reason: String,
    },
    Revoke {
        reason: String,
    },
    Cancel {
        reason: String,
    },
    Expire,
    Consume,
    Complete {
        outcome: ExecutionOutcome,
        reason: String,
    },
    RecoverUnknown,
    Reconcile {
        outcome: ExecutionOutcome,
        reason: String,
        actor: String,
    },
}

/// A synchronous single-writer store. No external call is made while changing
/// state. Keep it in trusted host code and serialize its short local operations.
pub struct ApprovalStore {
    file: File,
    _lock: File,
    config: ApprovalStoreConfig,
    sequence: u64,
    bytes: u64,
    poisoned: bool,
    records: BTreeMap<String, ApprovalRecord>,
    identity: Arc<()>,
}

impl ApprovalStore {
    pub fn get(&self, request_id: &str) -> Option<&ApprovalRecord> {
        self.records.get(request_id)
    }
    pub fn list(&self) -> Vec<ApprovalRecord> {
        self.records.values().cloned().collect()
    }

    /// Project bounded read views without copying stored action bodies.
    #[cfg(feature = "server")]
    pub(crate) fn map_records<T>(&self, projection: impl FnMut(&ApprovalRecord) -> T) -> Vec<T> {
        self.records.values().map(projection).collect()
    }

    pub fn request(
        &mut self,
        operation: ProposedOperation,
        policy: ApprovalPolicy,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.available()?;
        operation.validate()?;
        policy.validate()?;
        if let Some(previous) = self.records.values().find(|record| {
            record.request.operation.task_id == operation.task_id
                && record.request.operation.operation_id == operation.operation_id
        }) {
            return if previous.request.operation == operation && previous.request.policy == policy {
                Ok(previous.clone())
            } else {
                Err(ApprovalError::Conflict)
            };
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ApprovalError::Capacity)?;
        let request = ApprovalRequest {
            request_id: format!("approval-{sequence:016x}"),
            operation,
            policy,
            created_at: now,
            expires_at: 0,
        };
        let request = ApprovalRequest {
            expires_at: now
                .checked_add(request.policy.ttl_secs)
                .ok_or(ApprovalError::Invalid("expiry overflow"))?,
            ..request
        };
        self.append(Event::Requested { request }, now)
    }

    pub fn assess(
        &mut self,
        request_id: &str,
        assessment: ModelAssessment,
        reviewer: ReviewerIdentity,
        current_policy: &ApprovalPolicy,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.check_current(request_id, current_policy, now)?;
        if assessment.request_id != request_id {
            return Err(ApprovalError::Conflict);
        }
        self.change(
            request_id,
            Change::Assess {
                assessment: ApprovalAssessment {
                    decision: assessment.decision,
                    reason: assessment.reason,
                    reviewer: AssessmentSource::Harness {
                        harness_id: reviewer.harness_id,
                        session_id: reviewer.session_id,
                    },
                },
            },
            now,
        )
    }

    /// Only a trusted interactive host/CLI handler may call this. `actor` is audit
    /// attribution, not authentication. Never derive it from a model response.
    pub fn decide_human(
        &mut self,
        request_id: &str,
        decision: ApprovalDecision,
        reason: String,
        actor: String,
        current_policy: &ApprovalPolicy,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.check_current(request_id, current_policy, now)?;
        self.change(
            request_id,
            Change::HumanDecision {
                assessment: ApprovalAssessment {
                    decision,
                    reason,
                    reviewer: AssessmentSource::Human { actor },
                },
            },
            now,
        )
    }

    /// Atomically persist one execution intent before returning its capability.
    /// The caller must additionally revalidate the current target/file state.
    pub fn consume(
        &mut self,
        request_id: &str,
        operation: &ProposedOperation,
        current_policy: &ApprovalPolicy,
        now: u64,
    ) -> Result<ExecutionPermit, ApprovalError> {
        self.check_current(request_id, current_policy, now)?;
        let record = self
            .records
            .get(request_id)
            .ok_or(ApprovalError::NotFound)?;
        if &record.request.operation != operation {
            return Err(ApprovalError::Conflict);
        }
        let record = self.change(request_id, Change::Consume, now)?;
        Ok(ExecutionPermit {
            request_id: request_id.into(),
            revision: record.revision,
            operation: record.request.operation,
            store_identity: self.identity.clone(),
        })
    }

    pub fn complete(
        &mut self,
        permit: ExecutionPermit,
        outcome: ExecutionOutcome,
        reason: String,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.available()?;
        let record = self
            .records
            .get(&permit.request_id)
            .ok_or(ApprovalError::NotFound)?;
        if !Arc::ptr_eq(&self.identity, &permit.store_identity)
            || record.revision != permit.revision
            || record.request.operation != permit.operation
        {
            return Err(ApprovalError::Conflict);
        }
        self.change(
            &permit.request_id,
            Change::Complete { outcome, reason },
            now,
        )
    }

    pub fn revoke(
        &mut self,
        request_id: &str,
        reason: String,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.change(request_id, Change::Revoke { reason }, now)
    }

    pub fn cancel(
        &mut self,
        request_id: &str,
        reason: String,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.change(request_id, Change::Cancel { reason }, now)
    }

    /// Record a host-observed reviewer failure without inventing model output.
    pub fn mark_waiting_human(
        &mut self,
        request_id: &str,
        reason: String,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.available()?;
        let policy = self
            .records
            .get(request_id)
            .ok_or(ApprovalError::NotFound)?
            .request
            .policy
            .clone();
        self.check_current(request_id, &policy, now)?;
        self.change(request_id, Change::WaitingHuman { reason }, now)
    }

    /// Call only after independently verifying the target and stopping the old
    /// executor. This records a terminal fact; it never renews an execution permit.
    pub fn reconcile_unknown(
        &mut self,
        request_id: &str,
        outcome: ExecutionOutcome,
        reason: String,
        actor: String,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.change(
            request_id,
            Change::Reconcile {
                outcome,
                reason,
                actor,
            },
            now,
        )
    }

    fn check_current(
        &mut self,
        id: &str,
        policy: &ApprovalPolicy,
        now: u64,
    ) -> Result<(), ApprovalError> {
        self.available()?;
        policy.validate()?;
        let record = self.records.get(id).ok_or(ApprovalError::NotFound)?;
        if &record.request.policy != policy {
            return Err(ApprovalError::Conflict);
        }
        if now < record.updated_at {
            return Err(ApprovalError::Invalid("clock moved backwards"));
        }
        if now >= record.request.expires_at
            && matches!(
                record.state,
                ApprovalState::Pending | ApprovalState::WaitingHuman | ApprovalState::Approved
            )
        {
            self.change(id, Change::Expire, now)?;
            return Err(ApprovalError::Expired);
        }
        if record.state == ApprovalState::Expired {
            return Err(ApprovalError::Expired);
        }
        Ok(())
    }

    fn available(&self) -> Result<(), ApprovalError> {
        if self.poisoned {
            Err(ApprovalError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn change(
        &mut self,
        id: &str,
        change: Change,
        now: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        self.append(
            Event::Changed {
                request_id: id.into(),
                change,
            },
            now,
        )
    }
}

fn text(value: &str, maximum: usize) -> Result<(), ApprovalError> {
    if value.trim().is_empty() || value.len() > maximum || value.contains('\0') {
        Err(ApprovalError::Invalid(
            "empty, oversized or NUL-containing text",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
use paths::validate_external_dir_for_source;

#[cfg(test)]
#[path = "../../tests/approval_paths.rs"]
mod path_tests;
