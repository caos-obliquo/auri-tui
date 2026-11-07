# auri — offline terminal control for the Kuba Auri DSP cable

A lightweight, offline, local terminal solution to toggle and apply DSP curves
to the Kuba Auri (CB5100) USB-C DSP cable — no browser, no script runtime.
Native Rust, ratatui-based TUI, pure vim motions (`h/j/k/l`), talking straight
to the system's USB/HID subsystem.

Independent implementation. **Not a fork** of
[open-auri](https://github.com/) — no source is copied from it; see
`THIRD_PARTY_NOTICES.md`.

## Workspace

| Crate | Description |
|---|---|
| `auri-codec` | Byte-exact `0xCB` wire codec: RBJ/AutoEq biquads → Q23/Q27 words → CRC-16 → 9 reports |
| `auri-gate` | Safety gate v2: the observed envelope of the 11 official presets, verdict before any write |
| `auri-transport` | HID transport: real cable (`hidapi`), recording fake, dry-run writer |
| `auri-cli` | The `auri` binary: `devices`, `show`, `write`, `neutral` |

## Safety invariants

These are not negotiable; every milestone keeps them:

1. **Nothing reaches the cable without the safety gate.** `--commit` runs
   `gate_for_write` first and refuses a curve the gate refuses. Dry-run is the
   default.
2. **A write is never retried.** The firmware validates the CRC and rejects a
   bad payload silently; resending only races the device.
3. **An abort never sends the commit.** Flash keeps the previous preset.
4. **Flash writes are spaced (2 s), counted, and warned about (≥50/day).**
   The chip's flash endurance is unknown.

The chip's limits are the observed envelope of the 11 official presets, not a
proof of hardware safety.

## Protocol in one paragraph

Payload = 220 bytes: 14-byte preamble, `int32` LE Q23 preamp, ten
`int32` LE Q27 sections in the order `[b0, b2, -a1, b1, -a2]` (`a0` implied),
CRC-16/CCITT-FALSE LE. Split into nine 31-byte reports, report ID `0xCB`, byte
0 = report index, tail zero-padded. Commit = `01 65 00 01 FF` zero-padded to 31.
Data writes land in RAM (live); only the commit hits flash. Designed for
96 kHz.

## Install

```sh
git clone https://github.com/caos-obliquo/auri-tui
cd auri-tui
cargo build --release
```

### udev rules (Linux)

Without these, Chromium/WebHID and this binary both fail to open the device
with a normal user:

```sh
sudo tee /etc/udev/rules.d/99-kuba-auri.rules >/dev/null <<'EOF'
# Generic USB permission (libusb / hidapi backends)
SUBSYSTEMS=="usb", ATTRS{idVendor}=="3302", ATTRS{idProduct}=="8902", MODE="0666", TAG+="uaccess"
# HID raw node — where the write actually lands
KERNEL=="hidraw*", ATTRS{idVendor}=="3302", ATTRS{idProduct}=="8902", MODE="0666", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then replug the cable. Verify with `ls -l /dev/hidraw*` and
`cargo run -p auri-cli -- devices`.

## Usage

```sh
# What can we see?
auri devices

# Dry-run: gate verdict + the nine frames, write nothing (the default)
auri neutral
auri write curve.json
auri show curve.json

# Write to the cable — only a curve the gate approves
auri write curve.json --commit
auri neutral --commit

# Judge under the yolo profile: class-2 reasons are waived, hard ones never are
auri write curve.json --yolo --commit
```

The verdict is printed before the bytes, always:

```
gate: PASS (standard profile), CRC 0x4fff
gate: REFUSED (standard profile)
  g:preamp = -0.2 (limit -0.5)
  d:prefix-peak at band 0 = -0.19999996779759982 (limit -0.20368875272918074)
```

A refused curve exits non-zero and writes nothing, `--commit` or not.

A `curve.json` is the same shape the web app serialises:

```json
{ "preampDb": -2.8, "eqBands": [ { "kind": "PK", "freqHz": 25, "gainDb": -0.8, "qFactor": 0.3 } ] }
```

Ten bands, exactly, in the order they must reach the wire: keep cuts before
boosts.

## Tests

```sh
cargo test --workspace          # unit + transport + gate
just corpus                     # regenerate the gate verdicts from the oracle
just parity                     # byte-exact parity vs the open-auri oracles
```

The parity oracles (`eq0xcb.reference.json`, `usb-wire-golden.json`,
`gate-verdicts.json`) are AGPL-3.0 artefacts of open-auri and are **not
vendored here**. Tests read them from a sibling `open-auri` checkout, or from
`AURI_FIXTURES_DIR` / `AURI_GATE_VERDICTS`, and skip when they are absent.
Current status:

- **M0** — all 11 official presets encode byte-for-byte identically to the
  reference implementation.
- **M1** — every frame of the real 2026-10-28 Windows/Edge hardware capture
  (13 writes, 130 frames) is reproduced exactly; dry-run invariants hold.
- **M2** — the safety gate matches the JavaScript oracle on 336 generated
  verdicts covering all 19 reason codes, in both profiles, on both grids, with
  19,544 numeric comparisons (worst relative delta 3.2e-15).
- **M3** — `--commit` is gated: the CLI prints the verdict, refuses a refused
  curve with a non-zero exit, and only then opens the cable. `neutral` writes
  the −0.5 dB preamp the official Neutro preset uses (CRC `0x4fff`).

## Licensing

This repository is **MIT**. Two things to know before you ship a binary:

- The **hidapi** library that `hidapi` links is GPL-3.0-or-later, which makes
  the combined work a GPL work when distributed. Use `--dry-run` locally all
  you like; if you redistribute, either comply with GPL or swap the `HidSink`
  for a permissively licensed backend (the trait is already abstract).
- The protocol documentation and test oracles come from **open-auri**
  (AGPL-3.0) and are referenced, not copied.
