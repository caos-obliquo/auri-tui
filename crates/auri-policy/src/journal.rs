//! The append-only write journal: what the cable is believed to hold.
//!
//! The cable never reports what it stores, so this file is the only memory.
//! It is append-only JSONL, one entry per write attempt, appended **after**
//! the transport settles: a write that failed midway appends
//! [`JournalStatus::Interrupted`], which is exactly what makes the next
//! session's loudness state "unknown".
//!
//! A journal failure warns and continues. It never re-sends anything — the
//! transport write it describes already happened, once.

use std::path::{Path, PathBuf};

use auri_codec::Band;
use auri_gate::{Profile, level_db};
use serde::{Deserialize, Serialize};

use crate::store;

/// How a write attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalStatus {
    /// The commit reached the cable and settled.
    Ok,
    /// The write failed midway: what the cable holds is unknown, and the next
    /// session must assume so.
    Interrupted,
    /// A real write that never committed (RAM-only preview): the flash still
    /// holds the previous curve.
    Simulated,
}

/// One journaled write attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    /// When the write attempt ended, epoch milliseconds.
    pub ts: i64,
    /// How it ended.
    pub status: JournalStatus,
    /// CRC the gate reported for this curve, as `0x` + hex.
    pub crc: String,
    /// The exact bands written — kept so `level_db` can be recomputed later.
    pub bands: Vec<Band>,
    /// The preamp written with them (dB).
    pub preamp_db: f64,
    /// Which profile approved the write.
    pub profile: Profile,
    /// The sink label the write went to.
    pub label: String,
}

/// Something the journal refuses to do silently.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// The journal could not be read (a missing file is *not* an error).
    #[error("cannot read {path}: {message}")]
    Read { path: PathBuf, message: String },
    /// The entry could not be appended. The write it describes already
    /// happened — the caller warns and moves on, never re-sending.
    #[error("cannot append to {path}: {message}")]
    Append { path: PathBuf, message: String },
    /// A line that is not a journal entry: the file's memory cannot be
    /// trusted, so nothing is inferred from it (unknown ⇒ conservative).
    #[error("{path} line {line} is not a journal entry")]
    Line { path: PathBuf, line: usize },
}

/// `~/.local/share/auri/journal.jsonl` (XDG), owner-only.
#[must_use]
pub fn journal_path() -> PathBuf {
    store::data_dir().join("journal.jsonl")
}

/// Appends one entry as a single JSON line. Call this exactly once per write
/// attempt, after the transport settles — never before, never twice.
///
/// # Errors
///
/// [`JournalError::Append`] on any I/O or serialisation problem. Callers warn
/// and continue: losing history is safe, sending again is not.
pub fn append(path: &Path, entry: &JournalEntry) -> Result<(), JournalError> {
    let mut line = serde_json::to_string(entry).map_err(|error| JournalError::Append {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    line.push('\n');
    store::append_private(path, line.as_bytes()).map_err(|error| JournalError::Append {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// Reads every entry, oldest first. A missing file is an empty journal; an
/// unreadable or malformed one is an error, because a partial read could make
/// a broken history look like a healthy one.
///
/// # Errors
///
/// [`JournalError::Read`] or [`JournalError::Line`]. Callers fall back to "no
/// entries", which is the conservative answer for every question asked here.
pub fn read(path: &Path) -> Result<Vec<JournalEntry>, JournalError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(JournalError::Read {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    };
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry = serde_json::from_str::<JournalEntry>(line).map_err(|_| JournalError::Line {
            path: path.to_path_buf(),
            line: index + 1,
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

/// The `level_db` of the newest curve that actually reached the cable: the
/// newest [`JournalStatus::Ok`] entry, skipping simulated and interrupted
/// ones. `None` when there is none (or its curve no longer encodes).
#[must_use]
pub fn last_written_level_db(entries: &[JournalEntry]) -> Option<f64> {
    entries
        .iter()
        .rev()
        .find(|entry| entry.status == JournalStatus::Ok)
        .and_then(|entry| level_db(&entry.bands, entry.preamp_db).ok())
        .filter(|level| level.is_finite())
}

/// The status of the newest entry, whatever it was — an
/// [`JournalStatus::Interrupted`] tail is how the checklist learns the last
/// session ended mid-write.
#[must_use]
pub fn last_status(entries: &[JournalEntry]) -> Option<JournalStatus> {
    entries.last().map(|entry| entry.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auri_codec::NEUTRAL_BANDS;

    fn entry(ts: i64, status: JournalStatus) -> JournalEntry {
        JournalEntry {
            ts,
            status,
            crc: "0x4fff".to_string(),
            bands: NEUTRAL_BANDS.to_vec(),
            preamp_db: -0.5,
            profile: Profile::Standard,
            label: "test sink".to_string(),
        }
    }

    fn temp_journal(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "auri-policy-journal-test-{}-{tag}.jsonl",
            std::process::id()
        ))
    }

    #[test]
    fn append_then_read_roundtrips_every_field() {
        let path = temp_journal("roundtrip");
        let _ = std::fs::remove_file(&path);

        append(&path, &entry(1_000, JournalStatus::Ok)).expect("append");
        append(&path, &entry(2_000, JournalStatus::Interrupted)).expect("append");

        let entries = read(&path).expect("read");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], entry(1_000, JournalStatus::Ok));
        assert_eq!(entries[1].status, JournalStatus::Interrupted);
        assert_eq!(entries[0].crc, "0x4fff");
        assert_eq!(entries[0].bands, NEUTRAL_BANDS.to_vec());
        assert_eq!(entries[0].profile, Profile::Standard);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_journal_is_empty_not_an_error() {
        let path = temp_journal("missing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read(&path).expect("missing reads empty"), Vec::new());
        assert_eq!(last_written_level_db(&[]), None, "no history, no level");
        assert_eq!(last_status(&[]), None);
    }

    #[test]
    fn last_written_level_skips_simulated_and_interrupted_entries() {
        let ok = entry(1_000, JournalStatus::Ok);
        let level = last_written_level_db(std::slice::from_ref(&ok))
            .expect("the neutral curve has a level");

        let mut later = entry(2_000, JournalStatus::Interrupted);
        later.preamp_db = -12.0;
        assert_eq!(
            last_written_level_db(&[ok.clone(), later.clone()]),
            Some(level),
            "the interrupted tail does not erase the ok write"
        );

        let mut simulated = entry(3_000, JournalStatus::Simulated);
        simulated.preamp_db = -25.5;
        assert_eq!(
            last_written_level_db(&[ok, later, simulated]),
            Some(level),
            "simulated writes never reached the flash"
        );

        assert_eq!(
            last_status(&[
                entry(1_000, JournalStatus::Ok),
                entry(2_000, JournalStatus::Interrupted)
            ]),
            Some(JournalStatus::Interrupted),
            "the newest entry's status is what the next session sees"
        );
    }

    #[test]
    fn a_malformed_line_is_refused_rather_than_partially_trusted() {
        let path = temp_journal("malformed");
        let _ = std::fs::remove_file(&path);
        append(&path, &entry(1_000, JournalStatus::Ok)).expect("append");
        store::append_private(&path, b"{not json\n").expect("append junk");
        assert!(matches!(
            read(&path),
            Err(JournalError::Line { line: 2, .. })
        ));
        let _ = std::fs::remove_file(&path);
    }
}
