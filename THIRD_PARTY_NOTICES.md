# Third-party notices

This project is an independent implementation of the `0xCB` wire format of the
Kuba Auri (CB5100) USB-C DSP cable. No source file is copied from the projects
below; their *algorithms and published constants* are reimplemented in Rust.

## open-auri — AGPL-3.0

<https://github.com/marco-jardim/open-auri> (local checkout `~/builds/open-auri`)

Protocol documentation (`docs/protocol-0xcb.md`), the byte layout of the
payload (preamble, Q23 preamp, Q27 sections in `[b0, b2, -a1, b1, -a2]` order,
CRC-16/CCITT framing, commit command), the safety-gate thresholds and the
byte-exact test oracles in `test/fixtures/` all originate there.

The oracles are **not vendored** into this MIT repository. Tests read them at
run time from a sibling `open-auri` checkout (override with
`AURI_FIXTURES_DIR`); the parity tests skip when the checkout is absent.

## Kuba — reference implementation

<https://equalizador.kuba.audio/>

The reference byte streams in `eq0xcb.reference.json` were produced by Kuba's
page DSP implementation. Presets and reference implementation: Kuba
(kuba.audio). The reference page attributes its default curve to Leonardo
("Padrão = filtros do Leonardo"). No code or names come from it.

## AutoEq — MIT

<https://github.com/jaakkopasanen/AutoEq> (`autoeq/peq.py`:
`Peaking.biquad_coefficients`, `LowShelf.biquad_coefficients`,
`HighShelf.biquad_coefficients`), Copyright (c) 2018-2022 Jaakko Pasanen.

Peaking, low-shelf and high-shelf design are ported from AutoEq, keeping
AutoEq's arithmetic order and its negated feedback-coefficient convention.

## Robert Bristow-Johnson — Audio EQ Cookbook

Low-pass and high-pass design follow the Audio EQ Cookbook directly (AutoEq
has no such filters).

## hidapi — BSD-3-Clause (chosen)

<https://github.com/libusb/hidapi> — linked as a native library by the
`hidapi` Rust crate. HIDAPI is tri-licensed and states that *"the license
chosen is at the discretion of the user of HIDAPI"*: GPL-3.0, BSD-3-Clause, or
the original HIDAPI license. This project uses it under **BSD-3-Clause**, so
distributed binaries stay permissive.

Copyright (c) 2010, Alan Ott, Signal 11 Software. All rights reserved.
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the BSD-3-Clause conditions are met
(the full text ships with the `hidapi` crate as `LICENSE-bsd.txt`).
