use thiserror::Error;

use super::model::MemoryHintStatus;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HintTransitionError {
    #[error("memory hint is tombstoned and cannot transition to {target}")]
    Tombstoned { target: &'static str },
    #[error("invalid memory hint transition from {from} to {to}")]
    Invalid {
        from: &'static str,
        to: &'static str,
    },
}

pub fn validate_hint_transition(
    from: MemoryHintStatus,
    to: MemoryHintStatus,
) -> Result<(), HintTransitionError> {
    if from == to {
        return Ok(());
    }

    if from == MemoryHintStatus::Tombstoned {
        return Err(HintTransitionError::Tombstoned {
            target: to.as_str(),
        });
    }

    let valid = match from {
        MemoryHintStatus::Candidate => matches!(
            to,
            MemoryHintStatus::Approved | MemoryHintStatus::Rejected | MemoryHintStatus::Tombstoned
        ),
        MemoryHintStatus::Approved
        | MemoryHintStatus::Pinned
        | MemoryHintStatus::Deprioritized
        | MemoryHintStatus::Suppressed => matches!(
            to,
            MemoryHintStatus::Approved
                | MemoryHintStatus::Pinned
                | MemoryHintStatus::Deprioritized
                | MemoryHintStatus::Suppressed
                | MemoryHintStatus::Stale
                | MemoryHintStatus::Tombstoned
        ),
        MemoryHintStatus::Rejected | MemoryHintStatus::Stale => {
            matches!(to, MemoryHintStatus::Tombstoned)
        }
        MemoryHintStatus::Tombstoned => false,
    };

    if valid {
        Ok(())
    } else {
        Err(HintTransitionError::Invalid {
            from: from.as_str(),
            to: to.as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_hint_transition, HintTransitionError};
    use crate::memory::model::MemoryHintStatus;

    #[test]
    fn allows_candidate_review_transitions() {
        validate_hint_transition(MemoryHintStatus::Candidate, MemoryHintStatus::Approved).unwrap();
        validate_hint_transition(MemoryHintStatus::Candidate, MemoryHintStatus::Rejected).unwrap();
        validate_hint_transition(MemoryHintStatus::Candidate, MemoryHintStatus::Tombstoned)
            .unwrap();
    }

    #[test]
    fn keeps_tombstone_terminal() {
        let error =
            validate_hint_transition(MemoryHintStatus::Tombstoned, MemoryHintStatus::Approved)
                .unwrap_err();
        assert_eq!(
            error,
            HintTransitionError::Tombstoned { target: "approved" }
        );
    }

    #[test]
    fn rejects_candidate_to_pinned_without_approval() {
        let error = validate_hint_transition(MemoryHintStatus::Candidate, MemoryHintStatus::Pinned)
            .unwrap_err();
        assert_eq!(
            error,
            HintTransitionError::Invalid {
                from: "candidate",
                to: "pinned"
            }
        );
    }
}
