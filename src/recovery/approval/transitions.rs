//! Pure validation of live and replayed approval state transitions.
use super::*;

impl ApprovalStore {
    pub(super) fn apply(
        &self,
        event: &Event,
        now: u64,
        sequence: u64,
    ) -> Result<ApprovalRecord, ApprovalError> {
        let (id, change) = match event {
            Event::Requested { request } => {
                request.operation.validate()?;
                request.policy.validate()?;
                if self.records.len() >= self.config.max_requests {
                    return Err(ApprovalError::Capacity);
                }
                if request.request_id != format!("approval-{sequence:016x}")
                    || request.created_at != now
                    || request.created_at.checked_add(request.policy.ttl_secs)
                        != Some(request.expires_at)
                    || self.records.values().any(|record| {
                        record.request.operation.task_id == request.operation.task_id
                            && record.request.operation.operation_id
                                == request.operation.operation_id
                    })
                {
                    return Err(ApprovalError::Conflict);
                }
                return Ok(ApprovalRecord {
                    request: request.clone(),
                    state: ApprovalState::Pending,
                    revision: 0,
                    updated_at: now,
                    assessment: None,
                    note: None,
                });
            }
            Event::Changed { request_id, change } => (request_id, change),
        };
        let mut record = self.records.get(id).ok_or(ApprovalError::NotFound)?.clone();
        if now < record.updated_at {
            return Err(ApprovalError::Invalid("clock moved backwards"));
        }
        let state = record.state;
        match change {
            Change::Assess { assessment } => {
                require_state(state, &[ApprovalState::Pending])?;
                fresh(&record, now)?;
                validate_assessment(assessment)?;
                match (&record.request.policy.reviewer, &assessment.reviewer) {
                    (
                        ReviewerConfig::Harness { harness_id },
                        AssessmentSource::Harness {
                            harness_id: actual, ..
                        },
                    ) if harness_id == actual => {}
                    _ => return Err(ApprovalError::Conflict),
                }
                record.state = decision_state(&record, assessment.decision)?;
                record.assessment = Some(assessment.clone());
            }
            Change::HumanDecision { assessment } => {
                require_state(
                    state,
                    &[ApprovalState::Pending, ApprovalState::WaitingHuman],
                )?;
                fresh(&record, now)?;
                validate_assessment(assessment)?;
                if !matches!(assessment.reviewer, AssessmentSource::Human { .. }) {
                    return Err(ApprovalError::Invalid("human reviewer required"));
                }
                record.state = decision_state(&record, assessment.decision)?;
                record.assessment = Some(assessment.clone());
            }
            Change::Revoke { reason } | Change::Cancel { reason } => {
                text(reason, MAX_REASON)?;
                require_state(
                    state,
                    &[
                        ApprovalState::Pending,
                        ApprovalState::WaitingHuman,
                        ApprovalState::Approved,
                        ApprovalState::Executing,
                    ],
                )?;
                record.state = if state == ApprovalState::Executing {
                    ApprovalState::Unknown
                } else if matches!(change, Change::Revoke { .. }) {
                    ApprovalState::Revoked
                } else {
                    ApprovalState::Canceled
                };
                record.note = Some(reason.clone());
            }
            Change::WaitingHuman { reason } => {
                require_state(state, &[ApprovalState::Pending])?;
                fresh(&record, now)?;
                text(reason, MAX_REASON)?;
                record.state = ApprovalState::WaitingHuman;
                record.note = Some(reason.clone());
            }
            Change::Expire => {
                require_state(
                    state,
                    &[
                        ApprovalState::Pending,
                        ApprovalState::WaitingHuman,
                        ApprovalState::Approved,
                    ],
                )?;
                if now < record.request.expires_at {
                    return Err(ApprovalError::Invalid("premature expiry"));
                }
                record.state = ApprovalState::Expired;
            }
            Change::Consume => {
                require_state(state, &[ApprovalState::Approved])?;
                fresh(&record, now)?;
                if !record.request.policy.allows(&record.request.operation) {
                    return Err(ApprovalError::OutOfScope);
                }
                if self.records.values().any(|other| {
                    other.request.operation.target == record.request.operation.target
                        && matches!(
                            other.state,
                            ApprovalState::Executing | ApprovalState::Unknown
                        )
                }) {
                    return Err(ApprovalError::TargetBusy);
                }
                record.state = ApprovalState::Executing;
            }
            Change::Complete { outcome, reason } => {
                require_state(state, &[ApprovalState::Executing])?;
                text(reason, MAX_REASON)?;
                record.state = outcome_state(*outcome);
                record.note = Some(reason.clone());
            }
            Change::RecoverUnknown => {
                require_state(state, &[ApprovalState::Executing])?;
                record.state = ApprovalState::Unknown;
                record.note = Some("host restarted with an unconfirmed execution intent".into());
            }
            Change::Reconcile {
                outcome,
                reason,
                actor,
            } => {
                require_state(state, &[ApprovalState::Unknown])?;
                if *outcome == ExecutionOutcome::Unknown {
                    return Err(ApprovalError::Invalid(
                        "reconciliation requires a verified terminal outcome",
                    ));
                }
                text(reason, MAX_REASON)?;
                text(actor, MAX_ID)?;
                record.state = outcome_state(*outcome);
                record.note = Some(format!("verified by {actor}: {reason}"));
            }
        }
        record.revision = record
            .revision
            .checked_add(1)
            .ok_or(ApprovalError::Capacity)?;
        record.updated_at = now;
        Ok(record)
    }
}

fn decision_state(
    record: &ApprovalRecord,
    decision: ApprovalDecision,
) -> Result<ApprovalState, ApprovalError> {
    match decision {
        ApprovalDecision::Approve if !record.request.policy.allows(&record.request.operation) => {
            Err(ApprovalError::OutOfScope)
        }
        ApprovalDecision::Approve => Ok(ApprovalState::Approved),
        ApprovalDecision::Deny => Ok(ApprovalState::Denied),
        ApprovalDecision::Escalate => Ok(ApprovalState::WaitingHuman),
    }
}

fn validate_assessment(assessment: &ApprovalAssessment) -> Result<(), ApprovalError> {
    text(&assessment.reason, MAX_REASON)?;
    match &assessment.reviewer {
        AssessmentSource::Harness {
            harness_id,
            session_id,
        } => {
            text(harness_id, MAX_ID)?;
            text(session_id, MAX_ID)
        }
        AssessmentSource::Human { actor } => text(actor, MAX_ID),
    }
}

fn fresh(record: &ApprovalRecord, now: u64) -> Result<(), ApprovalError> {
    if now >= record.request.expires_at {
        Err(ApprovalError::Expired)
    } else {
        Ok(())
    }
}

fn require_state(state: ApprovalState, allowed: &[ApprovalState]) -> Result<(), ApprovalError> {
    if allowed.contains(&state) {
        Ok(())
    } else {
        Err(ApprovalError::InvalidState(state))
    }
}

fn outcome_state(outcome: ExecutionOutcome) -> ApprovalState {
    match outcome {
        ExecutionOutcome::Executed => ApprovalState::Executed,
        ExecutionOutcome::Failed => ApprovalState::Failed,
        ExecutionOutcome::Unknown => ApprovalState::Unknown,
    }
}
