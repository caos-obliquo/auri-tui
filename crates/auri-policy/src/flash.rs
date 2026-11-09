//! Flash-wear bookkeeping: how often the cable's memory was written, and
//! whether the next write may go now.
//!
//! Pure functions with an injected clock — never call `now()` here. Port of
//! the behaviour of open-auri's `src/policy/flash.mjs` (AGPL-3.0); only the
//! algorithm is re-derived, the prose is not reused.
//!
//! Rules that matter:
//! - A `None` timestamp means **unknown**, never "no cooldown": the full
//!   [`MIN_INTERVAL_MS`] wait applies.
//! - Counters saturate rather than wrap, and never block a write.
//! - Only a committed write is recorded: RAM-only writes do not count and do
//!   not start the cooldown.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store;

/// Minimum spacing between two commits to flash, in milliseconds.
pub const MIN_INTERVAL_MS: i64 = 2000;

/// Writes per local day at which the endurance notice starts showing.
pub const DAILY_NOTICE_THRESHOLD: u64 = 50;

/// Largest epoch millisecond a JavaScript `Date` can represent; timestamps
/// beyond it are treated as unknown here too.
const MAX_TIMESTAMP_MS: i64 = 8_640_000_000_000_000;

/// The endurance notice, re-authored (the reference's Portuguese copy is AGPL).
pub const FLASH_NOTICE_TEXT: &str =
    "Flash endurance is unknown: memories like this usually survive between 10 thousand and 1 million writes.";

/// Something the flash policy refuses to guess.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlashError {
    /// The injected clock is not a usable epoch millisecond.
    #[error("clock {0} ms is not a valid epoch millisecond")]
    InvalidClock(i64),
    /// A date key that is not a real Gregorian date.
    #[error("date key {0:?} is not a real YYYY-MM-DD date")]
    InvalidDateKey(String),
    /// The state file could not be written.
    #[error("cannot write {path}: {message}")]
    Save { path: PathBuf, message: String },
}

/// One local day of writes. `date` is always a validated `YYYY-MM-DD`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlashDay {
    /// The local calendar day (validated on parse).
    pub date: String,
    /// Writes recorded that day.
    pub count: u64,
}

/// The persisted flash counters.
///
/// Serialised as one JSON object (`day` may be `null`); a file that cannot be
/// read or parsed yields [`FlashState::default`] — unknown ⇒ full wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FlashState {
    /// Today's writes; `None` is a reset day.
    #[serde(default)]
    pub day: Option<FlashDay>,
    /// Writes since the counters began.
    #[serde(default)]
    pub total: u64,
    /// Epoch ms of the last recorded write; `None` means **unknown**.
    #[serde(default)]
    pub last_write_at: Option<i64>,
    /// Writes this process. Loaded files always restart it at zero, like a
    /// fresh browser session: "this session" means this run.
    #[serde(default)]
    pub session: u64,
}

/// Whether a write may go now, and how long to wait if not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spacing {
    /// `true` when the full interval has already elapsed.
    pub ok: bool,
    /// Milliseconds left before the next write (`0` when `ok`).
    pub wait_ms: u64,
}

/// The counters one screen shows: session, local day, total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// Writes this process.
    pub session: u64,
    /// Writes recorded on the local date of `now`.
    pub day: u64,
    /// Writes since the counters began.
    pub total: u64,
}

/// Whether `value` is a timestamp the reference would accept.
const fn is_timestamp(value: i64) -> bool {
    value >= 0 && value <= MAX_TIMESTAMP_MS
}

/// Whether `value` is a real Gregorian date in `YYYY-MM-DD` — shape **and**
/// calendar, leap years included.
#[must_use]
pub fn is_date_key(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |part: &[u8]| part.iter().all(u8::is_ascii_digit);
    if !(digits(&bytes[0..4]) && digits(&bytes[5..7]) && digits(&bytes[8..10])) {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u32>() else {
        return false;
    };
    if !(1..=12).contains(&month) {
        return false;
    }
    const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let mut days = DAYS[(month - 1) as usize];
    if month == 2 && leap {
        days = 29;
    }
    (1..=days).contains(&day)
}

/// The **local** calendar date of `now_ms` (not UTC), as `YYYY-MM-DD`.
///
/// # Errors
///
/// [`FlashError::InvalidClock`] when `now_ms` is not a usable epoch
/// millisecond, or when the local date does not fit the key.
pub fn local_date_key(now_ms: i64) -> Result<String, FlashError> {
    if !is_timestamp(now_ms) {
        return Err(FlashError::InvalidClock(now_ms));
    }
    let timestamp = chrono::DateTime::from_timestamp_millis(now_ms)
        .ok_or(FlashError::InvalidClock(now_ms))?;
    let key = timestamp
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string();
    if !is_date_key(&key) {
        return Err(FlashError::InvalidDateKey(key));
    }
    Ok(key)
}

/// Anchors `state` at `now_ms`.
///
/// An unknown timestamp — or a clock that jumped behind the last write — is
/// rebased to this observation, "as if just written", so every backwards jump
/// starts one fresh, bounded wait instead of waiting for the old clock to
/// catch up. With a monotonic clock this is the identity.
#[must_use]
pub fn observe_flash_state(state: &FlashState, now_ms: i64) -> FlashState {
    let last_write_at = match state.last_write_at {
        Some(previous) if is_timestamp(previous) && now_ms >= previous => Some(previous),
        _ => Some(now_ms),
    };
    FlashState {
        last_write_at,
        ..state.clone()
    }
}

/// Whether a write may go now: an unknown timestamp, a negative elapsed time
/// or a clock jump backwards all mean the **full** [`MIN_INTERVAL_MS`] wait.
///
/// This only asks; it never mutates. Persist [`observe_flash_state`] once at
/// startup so the anchor survives repeated polls.
#[must_use]
pub fn can_write_now(state: &FlashState, now_ms: i64) -> Spacing {
    let anchor = state.last_write_at.filter(|&value| is_timestamp(value));
    let elapsed = anchor.map_or(0, |previous| now_ms.saturating_sub(previous).max(0));
    let wait_ms = (MIN_INTERVAL_MS - elapsed).max(0);
    Spacing {
        ok: wait_ms == 0,
        wait_ms: u64::try_from(wait_ms).unwrap_or(u64::MAX),
    }
}

/// Records an actual write (not a permission check — the caller must have
/// honoured [`can_write_now`]): the day rolls over when the local date
/// changed, every counter increments with saturation (never a wrap, never a
/// limit), and the cooldown restarts at `now_ms`.
///
/// # Errors
///
/// [`FlashError::InvalidClock`] when `now_ms` has no local date.
pub fn record_write(state: &FlashState, now_ms: i64) -> Result<FlashState, FlashError> {
    let date = local_date_key(now_ms)?;
    record_write_on(state, now_ms, &date)
}

/// [`record_write`] with the date key supplied (tests roll days without
/// waiting for midnight).
///
/// # Errors
///
/// [`FlashError::InvalidDateKey`] when `date` is not a real date.
pub fn record_write_on(
    state: &FlashState,
    now_ms: i64,
    date: &str,
) -> Result<FlashState, FlashError> {
    if !is_date_key(date) {
        return Err(FlashError::InvalidDateKey(date.to_string()));
    }
    let count = match &state.day {
        Some(day) if day.date == date => day.count,
        _ => 0,
    };
    Ok(FlashState {
        day: Some(FlashDay {
            date: date.to_string(),
            count: count.saturating_add(1),
        }),
        total: state.total.saturating_add(1),
        last_write_at: Some(now_ms),
        session: state.session.saturating_add(1),
    })
}

/// Whether today's write count has reached [`DAILY_NOTICE_THRESHOLD`]. An
/// uncomputable date shows nothing rather than blocking anything.
#[must_use]
pub fn daily_notice(state: &FlashState, now_ms: i64) -> bool {
    local_date_key(now_ms).is_ok_and(|date| daily_notice_on(state, &date))
}

/// [`daily_notice`] with the date key supplied.
#[must_use]
pub fn daily_notice_on(state: &FlashState, date: &str) -> bool {
    state
        .day
        .as_ref()
        .is_some_and(|day| day.date == date && day.count >= DAILY_NOTICE_THRESHOLD)
}

/// The counters the checklist shows; a day that is not today counts as 0.
#[must_use]
pub fn counters(state: &FlashState, now_ms: i64) -> Counters {
    let today = local_date_key(now_ms).unwrap_or_default();
    counters_on(state, &today)
}

/// [`counters`] with the date key supplied.
#[must_use]
pub fn counters_on(state: &FlashState, today: &str) -> Counters {
    Counters {
        session: state.session,
        day: state
            .day
            .as_ref()
            .map_or(0, |day| if day.date == today { day.count } else { 0 }),
        total: state.total,
    }
}

/// `~/.local/share/auri/flash.json` (XDG), owner-only.
#[must_use]
pub fn flash_path() -> PathBuf {
    store::data_dir().join("flash.json")
}

/// Loads the persisted state, anchored at `now_ms`.
///
/// Any read or parse problem yields a fresh state — conservative, because
/// unknown means the full wait. `session` always restarts at 0: "this
/// session" belongs to this process, not to the file.
#[must_use]
pub fn load_flash(path: &Path, now_ms: i64) -> FlashState {
    let stored = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<FlashState>(&text).ok())
        .unwrap_or_default();
    observe_flash_state(
        &FlashState {
            session: 0,
            ..stored
        },
        now_ms,
    )
}

/// Persists the state.
///
/// # Errors
///
/// [`FlashError::Save`] when the file cannot be written. Callers warn and
/// continue: losing a counter is safe, retrying a write is not.
pub fn save_flash(path: &Path, state: &FlashState) -> Result<(), FlashError> {
    let json = serde_json::to_string_pretty(state)
        .map_err(|error| FlashError::Save {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    store::write_private(path, json.as_bytes()).map_err(|error| FlashError::Save {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(now: i64) -> FlashState {
        observe_flash_state(&FlashState::default(), now)
    }

    #[test]
    fn date_keys_validate_shape_and_calendar() {
        assert!(is_date_key("2026-11-09"));
        assert!(is_date_key("2024-02-29"), "leap day of a leap year");
        assert!(!is_date_key("2023-02-29"), "not a leap year");
        assert!(!is_date_key("1900-02-29"), "century rule");
        assert!(is_date_key("2000-02-29"), "400-year rule");
        assert!(!is_date_key("2026-13-01"));
        assert!(!is_date_key("2026-00-10"));
        assert!(!is_date_key("2026-04-31"));
        assert!(!is_date_key("26-11-09"));
        assert!(!is_date_key("2026/11/09"));
        assert!(!is_date_key("20261109"));
        assert!(!is_date_key(""));
    }

    #[test]
    fn local_date_key_is_local_and_valid() {
        let key = local_date_key(0).expect("epoch is a valid clock");
        assert!(is_date_key(&key), "{key}");
        // Mid-day UTC on a known date: the local date must be one of that UTC
        // date or its neighbour, never something else entirely.
        let noon = 1_767_225_600_000_i64; // 2026-01-01T00:00:00Z
        let key = local_date_key(noon).expect("valid clock");
        assert!(key == "2025-12-31" || key == "2026-01-01", "{key}");
        assert!(local_date_key(-1).is_err(), "before the epoch is refused");
        assert!(local_date_key(MAX_TIMESTAMP_MS + 1).is_err());
    }

    #[test]
    fn unknown_timestamp_waits_the_full_interval() {
        let state = FlashState::default(); // last_write_at: None
        let spacing = can_write_now(&state, 1_000_000);
        assert!(!spacing.ok);
        assert_eq!(spacing.wait_ms, 2000, "unknown never means no cooldown");
    }

    #[test]
    fn the_interval_expires_exactly_at_2000_ms_not_before() {
        let state = FlashState {
            last_write_at: Some(10_000),
            ..FlashState::default()
        };

        let early = can_write_now(&state, 11_999);
        assert!(!early.ok);
        assert_eq!(early.wait_ms, 1);

        let exact = can_write_now(&state, 12_000);
        assert!(exact.ok, "exactly 2000 ms elapsed");
        assert_eq!(exact.wait_ms, 0);

        let later = can_write_now(&state, 60_000);
        assert!(later.ok);
    }

    #[test]
    fn a_clock_that_went_backwards_waits_the_full_interval() {
        let state = FlashState {
            last_write_at: Some(50_000),
            ..FlashState::default()
        };
        let spacing = can_write_now(&state, 40_000);
        assert!(!spacing.ok, "negative elapsed must not mean ready");
        assert_eq!(spacing.wait_ms, 2000, "never a wait until the old clock");
    }

    #[test]
    fn observing_rebases_a_backwards_clock_forward() {
        let state = FlashState {
            last_write_at: Some(50_000),
            ..FlashState::default()
        };
        let observed = observe_flash_state(&state, 40_000);
        assert_eq!(observed.last_write_at, Some(40_000));
        // …and the fresh anchor now expires 2000 ms later, not at 52_000.
        assert!(!can_write_now(&observed, 41_999).ok);
        assert!(can_write_now(&observed, 42_000).ok);
        // Observing again with a monotonic clock is the identity.
        let again = observe_flash_state(&observed, 41_000);
        assert_eq!(again.last_write_at, Some(40_000));
        // An unknown timestamp is anchored by the observation too.
        let anchored = observe_flash_state(&FlashState::default(), 7_000);
        assert_eq!(anchored.last_write_at, Some(7_000));
    }

    #[test]
    fn record_write_counts_once_and_restarts_the_cooldown() {
        let state = fresh(0);
        let next = record_write(&state, 1_500).expect("valid clock");
        assert_eq!(next.session, 1);
        assert_eq!(next.total, 1);
        assert_eq!(next.day.as_ref().map(|d| d.count), Some(1));
        assert_eq!(next.last_write_at, Some(1_500));
        assert!(!can_write_now(&next, 2_000).ok);
        assert!(can_write_now(&next, 3_500).ok);

        let third = record_write(&next, 3_500).expect("valid clock");
        assert_eq!(third.session, 2);
        assert_eq!(third.total, 2);
        assert_eq!(third.day.as_ref().map(|d| d.count), Some(2), "same day keeps counting");
    }

    #[test]
    fn record_write_rolls_the_day_and_resets_only_the_day() {
        let state = FlashState {
            day: Some(FlashDay {
                date: "2026-11-08".to_string(),
                count: 49,
            }),
            total: 49,
            last_write_at: Some(1),
            session: 49,
        };
        let rolled = record_write_on(&state, 2_000, "2026-11-09").expect("real date");
        assert_eq!(
            rolled.day,
            Some(FlashDay {
                date: "2026-11-09".to_string(),
                count: 1
            }),
            "a new day starts at 1"
        );
        assert_eq!(rolled.total, 50, "the total never rolls");
        assert_eq!(rolled.session, 50, "the session never rolls");
        assert!(
            record_write_on(&state, 0, "2026-02-30").is_err(),
            "an impossible date never counts"
        );
    }

    #[test]
    fn counters_saturate_instead_of_wrapping() {
        let state = FlashState {
            day: Some(FlashDay {
                date: "2026-11-09".to_string(),
                count: u64::MAX,
            }),
            total: u64::MAX,
            last_write_at: Some(0),
            session: u64::MAX,
        };
        let next = record_write_on(&state, 1, "2026-11-09").expect("real date");
        assert_eq!(next.day.as_ref().map(|d| d.count), Some(u64::MAX));
        assert_eq!(next.total, u64::MAX);
        assert_eq!(next.session, u64::MAX, "saturate, never cap or wrap");
    }

    #[test]
    fn daily_notice_fires_at_50_not_at_49() {
        let state = FlashState {
            day: Some(FlashDay {
                date: "2026-11-09".to_string(),
                count: 49,
            }),
            ..FlashState::default()
        };
        assert!(!daily_notice_on(&state, "2026-11-09"));
        let fifty = FlashState {
            day: Some(FlashDay {
                date: "2026-11-09".to_string(),
                count: 50,
            }),
            ..FlashState::default()
        };
        assert!(daily_notice_on(&fifty, "2026-11-09"));
        assert!(
            !daily_notice_on(&fifty, "2026-11-10"),
            "yesterday's count is not today's notice"
        );
        assert!(
            !daily_notice_on(&FlashState::default(), "2026-11-09"),
            "no day, no notice"
        );
    }

    #[test]
    fn counters_only_count_today_for_today() {
        let state = FlashState {
            day: Some(FlashDay {
                date: "2026-11-08".to_string(),
                count: 40,
            }),
            total: 70,
            last_write_at: Some(0),
            session: 3,
        };
        let today = counters_on(&state, "2026-11-09");
        assert_eq!(
            today,
            Counters {
                session: 3,
                day: 0,
                total: 70
            },
            "yesterday's 40 are not today's"
        );
        assert_eq!(counters_on(&state, "2026-11-08").day, 40);
    }

    #[test]
    fn load_of_a_broken_file_is_fresh_but_anchored_and_restarts_the_session() {
        let path = std::env::temp_dir().join(format!(
            "auri-policy-flash-test-{}-missing.json",
            std::process::id()
        ));
        let state = load_flash(&path, 5_000);
        assert_eq!(state.last_write_at, Some(5_000), "anchored at the observation");
        assert_eq!(state.session, 0);
        assert_eq!(state.total, 0);
        assert!(!can_write_now(&state, 5_000).ok, "fresh still waits");

        // A corrupt file behaves exactly like no file.
        let corrupt = std::env::temp_dir().join(format!(
            "auri-policy-flash-test-{}-corrupt.json",
            std::process::id()
        ));
        std::fs::write(&corrupt, "not json at all").expect("temp file");
        let state = load_flash(&corrupt, 9_000);
        assert_eq!(state, FlashState { last_write_at: Some(9_000), ..FlashState::default() });
        let _ = std::fs::remove_file(&corrupt);
    }

    #[test]
    fn save_and_load_roundtrip_keeps_every_counter() {
        let path = std::env::temp_dir().join(format!(
            "auri-policy-flash-test-{}-roundtrip.json",
            std::process::id()
        ));
        let mut state = observe_flash_state(&FlashState::default(), 1_000);
        state = record_write(&state, 1_000).expect("valid clock");
        state = record_write(&state, 3_000).expect("valid clock");
        save_flash(&path, &state).expect("temp dir is writable");

        let loaded = load_flash(&path, 4_000);
        assert_eq!(loaded.total, 2);
        assert_eq!(loaded.day.as_ref().map(|d| d.count), Some(2));
        assert_eq!(loaded.last_write_at, Some(3_000), "the cooldown survives");
        assert_eq!(loaded.session, 0, "the session always restarts");
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn the_state_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "auri-policy-flash-test-{}-mode.json",
            std::process::id()
        ));
        save_flash(&path, &FlashState::default()).expect("temp dir is writable");
        let mode = std::fs::metadata(&path)
            .expect("file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "counters are nobody else's business");
        let _ = std::fs::remove_file(&path);
    }
}
