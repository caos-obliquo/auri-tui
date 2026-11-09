//! Pre-write policy for the Kuba Auri: how many times the flash was written,
//! whether two writes are far enough apart, what the "before writing"
//! checklist says, and the journal that remembers what the cable holds.
//!
//! The cable never reports what it stored — this crate's files are the only
//! memory, so a failed journal append is a warning, never a reason to send
//! anything twice.
//!
//! Behaviour is derived from the open-auri reference (AGPL-3.0): only the
//! algorithms are re-derived here, never the prose. Every function that reads
//! the clock takes it as an argument ([`now_ms`] is the one convenience for
//! callers), so the whole crate is unit-testable without sleeping.
//!
//! # Invariants
//!
//! - Only a committed write counts: [`flash::record_write`] is called after a
//!   `commit = true` write reaches the transport, never for a dry run.
//! - A journal failure never re-sends ([`journal::append`] is called exactly
//!   once per write attempt, after the transport is done).
//! - An unknown timestamp means the *full* spacing wait, never "no cooldown".

mod checklist;
mod flash;
mod journal;
mod loudness;
mod store;

pub use checklist::{
    ChecklistContext, ChecklistInput, ChecklistItem, ChecklistResult, Confirmations,
    CriterionValue, EXPERIMENTAL_TEXT, ItemId, LAMPS, LOCKED_TEXT, Lamp, LampId, LampState,
    ULTRASONIC_TEXT, criteria_values, evaluate_checklist, experimental_text,
    has_experimental_kinds, has_ultrasonic_bands, lamp_of, lamps_from_gate,
};
pub use flash::{
    Counters, DAILY_NOTICE_THRESHOLD, FLASH_NOTICE_TEXT, FlashDay, FlashError, FlashState,
    MIN_INTERVAL_MS, Spacing, can_write_now, counters, counters_on, daily_notice, daily_notice_on,
    flash_path, is_date_key, load_flash, local_date_key, observe_flash_state, record_write,
    record_write_on, save_flash,
};
pub use journal::{
    JournalEntry, JournalError, JournalStatus, append as journal_append, journal_path, last_status,
    last_written_level_db, read as journal_read,
};
pub use loudness::{
    LOUDNESS_JUMP_DB, LOUDNESS_TOLERANCE_DB, LOUDNESS_WARNING_TEXT, Loudness, LoudnessInput,
    LoudnessReason, loudness_warning,
};
pub use store::data_dir;

/// The current wall clock, epoch milliseconds. The one non-injected clock in
/// the workspace: binaries call this and pass the result down; tests never do.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}
