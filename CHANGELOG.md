# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `auri-codec`: byte-exact `0xCB` wire codec — RBJ/AutoEq biquads, Q23/Q27
  words, CRC-16/CCITT, nine reports.
- `auri-gate`: safety gate v2 with the observed envelope of the official
  presets, standard and yolo profiles, and 19 reason codes.
- `auri-policy`: flash counters and 2 s spacing, loudness-jump warning, an
  append-only JSONL journal, and the pre-write checklist.
- `auri-transport`: HID writer with per-report gap, settle delay, and the
  invariant that an abort never sends the commit.
- `auri` CLI: `devices`, `presets`, `show`, `write`, `neutral`, `tui`;
  dry-run by default, `--commit` gated, `--yolo` explicit.
- The 11 official Kuba Auri curves plus Neutro, built in: `auri presets` lists
  them, `--preset <id>` loads one (file and preset conflict).
- Interactive editor: vim motions, live gate verdict, braille response plot,
  arm-then-write, and the checklist (Tab/Space).
- `docs/udev.md`, a release workflow, and this changelog.

### Fixed

- The TUI no longer panics when it cannot open a terminal (`try_init`, with a
  clean error and terminal restore).
- Ctrl/Alt chords can never arm or write a curve; Ctrl+C quits.

### Security

- Nothing reaches the cable without the gate; writes are never retried; an
  abort never sends the commit.
