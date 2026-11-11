# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.0] - 2026-11-11

### Changed

- The TUI was re-skinned: one accent colour (`Color::Indexed(208)`) for the
  response curve, the selected band and its node, green only for a `PASS` and
  red only for a `REFUSED`. Thin rules and letter-spaced section labels
  replace the boxes, the key hints moved out of the titles, every band shows a
  gain bar centred on 0 dB, and the response is drawn as a line with an axis,
  decade marks and one node per band. `auri tui --ascii` draws the same
  screen with plain ASCII only.
- The HID transport no longer links `libudev`: the cable is found and opened
  through `/dev/hidraw*` and sysfs, in safe Rust. `cargo build --release
  --target x86_64-unknown-linux-musl` now produces a fully static binary
  (`ldd` reports `statically linked`), and the release publishes it next to
  the glibc one as `auri-linux-x86_64-musl`. The device label the journal
  records is unchanged.

### Added

- `auri-codec`: byte-exact `0xCB` wire codec — RBJ/AutoEq biquads, Q23/Q27
  words, CRC-16/CCITT, nine reports.
- `auri-gate`: safety gate v2 with the observed envelope of the official
  presets, standard and yolo profiles, and 19 reason codes.
- `auri-policy`: flash counters and 2 s spacing, loudness-jump warning, an
  append-only JSONL journal, and the pre-write checklist.
- `auri-transport`: HID writer with per-report gap, settle delay, and the
  invariant that an abort never sends the commit.
- `auri` CLI: `devices`, `presets`, `show`, `export`, `write`, `neutral`, `tui`;
  dry-run by default, `--commit` gated, `--yolo` explicit.
- The 11 official Kuba Auri curves plus Neutro, built in: `auri presets` lists
  them, `--preset <id>` loads one (file and preset conflict, also on `show`).
- Interactive editor: vim motions, `{`/`}` preset cycling with a `[RO]`
  read-only marker (any edit clears it — and disarms), live gate verdict,
  braille response plot, arm-then-write, and the checklist (Tab/Space).
- Read-only journal log (`L`) and help page (`?`): the help page lists every
  binding in one `KEY | CONTEXT | COMMAND` page and closes with `Esc`, `q` or
  `?`; it never arms, edits or writes.
- `auri export --preset <id> --format apo|pipewire`: host spellings of a
  curve, offline, no cable involved.
- `docs/udev.md`, a release workflow, and this changelog.

### Fixed

- The TUI no longer panics when it cannot open a terminal (`try_init`, with a
  clean error and terminal restore).
- Ctrl/Alt chords can never arm or write a curve; Ctrl+C quits.
- A closed stdout (`auri presets | head -1`) no longer panics on EPIPE; the
  `out`/`err` helpers buffer and ignore write errors.
- The first-ever `--commit` could never succeed: a refused write returned
  before `save_flash`, so the spacing anchor was never persisted and every
  fresh invocation re-anchored it to its own start time. The anchor is now
  written before the spacing check — only the timestamp, no counter moves,
  nothing re-sent.
- The TUI gain column printed its padding instead of its sign: `{:+>8.1}` makes
  `+` the *fill* character, so a `+2.5 dB` boost rendered as `+++++2.5dB` and
  a cut as `++++-3.0dB`. The sign flag now comes first (`{:>+8.1}`) — the
  column keeps its width, its alignment, and its sign.

### Security

- Nothing reaches the cable without the gate; writes are never retried; an
  abort never sends the commit.
