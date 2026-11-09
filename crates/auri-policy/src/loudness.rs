//! The loudness warning: this curve may sound louder than the one the cable
//! holds. Advisory only — it never blocks a write.
//!
//! Port of the behaviour of open-auri's `src/policy/loudness.mjs`
//! (AGPL-3.0); the levels themselves are supplied by the caller
//! ([`auri_gate::level_db`]), never computed here.

use crate::journal::JournalStatus;

/// How much louder (dB) the new curve may be before the warning lights.
pub const LOUDNESS_JUMP_DB: f64 = 3.0;

/// Headroom over [`LOUDNESS_JUMP_DB`] for Q23 rounding.
///
/// Over the allowed preamp range (−25.5 … −0.5 dB) the worst rounding is at
/// `word_min = round(10 ** (-25.5 / 20) * 2 ** 23)`: each level rounds by at
/// most half a word, so the difference error is bounded by
/// `2 * 20 * log10(1 + 0.5 / word_min)` ≈ 1.95e-5 dB. This tolerance sits
/// comfortably above that bound and far below audibility.
pub const LOUDNESS_TOLERANCE_DB: f64 = 1e-4;

/// The warning sentence, re-authored (the reference's Portuguese copy is AGPL).
pub const LOUDNESS_WARNING_TEXT: &str =
    "The new curve may sound louder. Lower the volume before writing.";

/// Why the warning lit, in stable order: jump first, then unknown state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoudnessReason {
    /// The new level is at least [`LOUDNESS_JUMP_DB`] above the last write.
    Jump,
    /// The last written level or the session's history is unknown, so no
    /// comparison could be made (first write, unreadable journal, or a write
    /// that was interrupted).
    UnknownState,
}

/// The warning decision.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loudness {
    /// Whether to show the warning at all.
    pub warn: bool,
    /// The lit reasons, jump before unknown-state.
    pub reasons: Vec<LoudnessReason>,
    /// `new − last`, present only when both levels and their difference are
    /// finite.
    pub delta_db: Option<f64>,
}

/// Everything [`loudness_warning`] compares. `None` levels are "unknown",
/// never zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessInput {
    /// The predicted level of the curve about to be written.
    pub new_level_db: Option<f64>,
    /// The level of the newest journaled write (`None` when there is none).
    pub last_written_level_db: Option<f64>,
    /// Writes recorded in this process. `0` means unknown state.
    pub session_write_count: u64,
    /// Status of the newest journal entry, however it ended.
    pub last_journal_status: Option<JournalStatus>,
}

/// The warning: `warn = jump || unknown_state`, with the reasons ordered
/// jump-first. A non-finite level never produces a delta, only unknown state.
#[must_use]
pub fn loudness_warning(input: LoudnessInput) -> Loudness {
    let new = input.new_level_db.unwrap_or(f64::NAN);
    let last = input.last_written_level_db.unwrap_or(f64::NAN);
    let difference = new - last;
    let finite = new.is_finite() && last.is_finite() && difference.is_finite();
    let delta_db = if finite { Some(difference) } else { None };

    let mut reasons = Vec::new();
    if finite && difference >= LOUDNESS_JUMP_DB - LOUDNESS_TOLERANCE_DB {
        reasons.push(LoudnessReason::Jump);
    }
    if !finite
        || input.session_write_count == 0
        || input.last_journal_status == Some(JournalStatus::Interrupted)
    {
        reasons.push(LoudnessReason::UnknownState);
    }
    Loudness {
        warn: !reasons.is_empty(),
        reasons,
        delta_db,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(new: Option<f64>, last: Option<f64>, session: u64) -> LoudnessInput {
        LoudnessInput {
            new_level_db: new,
            last_written_level_db: last,
            session_write_count: session,
            last_journal_status: Some(JournalStatus::Ok),
        }
    }

    #[test]
    fn a_jump_of_three_decibels_warns_and_smaller_does_not() {
        let small = loudness_warning(input(Some(-4.1), Some(-7.0), 1));
        assert!(!small.warn, "delta +2.9 dB is under the jump");
        assert!(
            small.delta_db.is_some_and(|delta| (delta - 2.9).abs() < 1e-12),
            "delta is +2.9 dB, got {:?}",
            small.delta_db
        );

        let exact = loudness_warning(input(Some(-1.1), Some(-4.1), 1));
        assert!(exact.warn, "delta +3.0 dB is a jump");
        assert_eq!(exact.reasons, vec![LoudnessReason::Jump]);
        assert!(
            exact.delta_db.is_some_and(|delta| (delta - 3.0).abs() < 1e-12),
            "delta is +3.0 dB, got {:?}",
            exact.delta_db
        );

        // Just inside the tolerance (3 − 1e-4) still warns…
        let edge = loudness_warning(input(Some(-4.1 + 2.999_95), Some(-4.1), 1));
        assert_eq!(
            edge.reasons,
            vec![LoudnessReason::Jump],
            "{:?}",
            edge.reasons
        );

        // …and a hair below it does not.
        let lower = loudness_warning(input(Some(-4.1 + 2.999_85), Some(-4.1), 1));
        assert!(!lower.warn, "below 3 − tolerance is not a jump");
    }

    #[test]
    fn the_first_write_of_a_session_has_unknown_state() {
        let first = loudness_warning(input(Some(-3.0), Some(-4.0), 0));
        assert!(first.warn, "nothing written yet means unknown");
        assert_eq!(first.reasons, vec![LoudnessReason::UnknownState]);
        assert_eq!(
            first.delta_db,
            Some(1.0),
            "unknown state does not erase a computable delta"
        );
    }

    #[test]
    fn missing_levels_are_unknown_never_zero() {
        let no_reference = loudness_warning(input(Some(-3.0), None, 4));
        assert!(no_reference.warn);
        assert_eq!(no_reference.reasons, vec![LoudnessReason::UnknownState]);
        assert_eq!(no_reference.delta_db, None, "no reference, no delta");

        let no_new = loudness_warning(input(None, Some(-4.0), 4));
        assert!(no_new.warn);
        assert_eq!(no_new.delta_db, None);

        let neither = loudness_warning(input(None, None, 0));
        assert_eq!(
            neither.reasons,
            vec![LoudnessReason::UnknownState],
            "one unknown-state reason, not two"
        );
    }

    #[test]
    fn an_interrupted_journal_entry_forces_unknown_state() {
        let mut case = input(Some(-1.0), Some(-4.0), 3);
        case.last_journal_status = Some(JournalStatus::Interrupted);
        let result = loudness_warning(case);
        assert!(result.warn);
        assert_eq!(
            result.reasons,
            vec![
                LoudnessReason::Jump,
                LoudnessReason::UnknownState
            ],
            "jump first, then unknown"
        );
    }

    #[test]
    fn a_quiet_happy_path_does_not_warn() {
        let result = loudness_warning(input(Some(-4.0), Some(-4.2), 2));
        assert!(!result.warn);
        assert!(result.reasons.is_empty());
        assert!(result.delta_db.is_some_and(|d| (d - 0.2).abs() < 1e-9));
    }
}
