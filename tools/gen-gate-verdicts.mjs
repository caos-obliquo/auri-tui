#!/usr/bin/env node
/**
 * Generates the gate parity corpus.
 *
 * Runs the reference `gate.mjs` (the source of truth) and the frozen
 * commit-807745d gate over a corpus of curves, and writes the verdicts to
 * `crates/auri-gate/tests/fixtures/gate-verdicts.json` for the Rust test
 * `crates/auri-gate/tests/parity.rs` to replay.
 *
 * The reference lives in the AGPL sibling repo and is never vendored here:
 * this script only *reads* it at generation time, and the generated JSON is
 * gitignored like every other oracle.
 *
 * Usage:
 *   node tools/gen-gate-verdicts.mjs
 *
 * Environment:
 *   AURI_OPEN_AURI_DIR  path to the open-auri checkout (default ../open-auri)
 */

import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..');
const OPEN_AURI = process.env.AURI_OPEN_AURI_DIR ?? resolve(REPO_ROOT, '..', 'open-auri');
const OUT = resolve(REPO_ROOT, 'crates/auri-gate/tests/fixtures/gate-verdicts.json');

const load = (relative) => import(pathToFileURL(resolve(OPEN_AURI, relative)).href);

const { gate } = await load('src/safety/gate.mjs');
const { gate: frozenGate } = await load('test/fixtures/gate-807745d.mjs');
const { OFFICIAL_PRESETS } = await load('src/catalog/presets.mjs');
const { NEUTRAL_BANDS } = await load('src/dsp/eq0xcb.mjs');

/** Every reason code the gate can emit. The corpus must hit all of them. */
const ALL_CODES = Object.freeze([
  'f:codec',
  'k:kind-not-allowed',
  'd7:pk-boost-above-16k',
  'd7:boost-q',
  'd7:boost-above-20k',
  'g:preamp',
  'a:b0',
  'a:b2',
  'a:-a1',
  'a:b1',
  'a:-a2',
  'b:pole-radius',
  'c:section-peak',
  'd:prefix-peak',
  'd:prefix-peak-full',
  'd:prefix-peak-extended',
  'e:output-audible',
  'e:output-extended',
  'e:output-full',
]);

const OPTIONS = Object.freeze([
  { experimental: false, grid: 'full', profile: 'standard' },
  { experimental: true, grid: 'full', profile: 'standard' },
  { experimental: false, grid: 'drag', profile: 'standard' },
  { experimental: false, grid: 'full', profile: 'yolo' },
  { experimental: true, grid: 'full', profile: 'yolo' },
]);

// --- curve helpers ----------------------------------------------------------

const OFF = Object.freeze({ kind: 'OFF' });

/** A10-band curve with `bands` in the leading slots and OFF filling the rest. */
function curve(bands) {
  const out = [];
  for (const band of bands) out.push(band);
  while (out.length < 10) out.push({ ...OFF });
  return out.slice(0, 10);
}

function pk(freqHz, gainDb, qFactor) {
  return { kind: 'PK', freqHz, gainDb, qFactor };
}
function shelf(kind, freqHz, gainDb, qFactor) {
  return { kind, freqHz, gainDb, qFactor };
}
function pass(kind, freqHz, qFactor) {
  return { kind, freqHz, qFactor };
}

/** Deterministic LCG so the corpus is byte-identical between runs. */
function makeRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

const round2 = (value) => Math.round(value * 100) / 100;

function randomBand(rnd) {
  const roll = rnd();
  if (roll < 0.12) return { ...OFF };
  const kind = ['PK', 'PK', 'PK', 'LS', 'LS', 'HS', 'LP', 'HP'][Math.floor(rnd() * 8)];
  const freqHz = round2(10 ** (rnd() * (Math.log10(40000) - Math.log10(20)) + Math.log10(20)));
  const qFactor = round2(10 ** (rnd() * (Math.log10(20) - Math.log10(0.1)) + Math.log10(0.1)));
  if (kind === 'LP' || kind === 'HP') return { kind, freqHz, qFactor };
  // Boosts are what stress the d/e limits, so keep them common.
  const gainDb = rnd() < 0.45 ? round2(rnd() * 9.2) : round2(-24 + rnd() * 24);
  return { kind, freqHz, gainDb, qFactor };
}

function randomCurve(rnd) {
  return curve(Array.from({ length: 10 }, () => randomBand(rnd)));
}

function randomUltrasonicCurve(rnd) {
  const bands = Array.from({ length: 10 }, () => randomBand(rnd));
  const at = Math.floor(rnd() * 10);
  const kind = ['PK', 'LS', 'HS'][Math.floor(rnd() * 3)];
  bands[at] = {
    kind,
    freqHz: round2(20001 + rnd() * 19999),
    gainDb: round2(-6 + rnd() * 15.2),
    qFactor: round2(0.1 + rnd() * 6),
  };
  return curve(bands);
}

// --- verdict serialisation --------------------------------------------------

function serialiseReason(reason) {
  const out = { code: reason.code };
  if (reason.stage !== undefined) out.stage = reason.stage;
  const value = reason.value;
  if (value === undefined || (typeof value === 'number' && !Number.isFinite(value))) {
    out.value = null;
  } else {
    out.value = value;
  }
  if (reason.limit !== undefined) out.limit = reason.limit;
  return out;
}

function serialisePeak(peak) {
  return { db: peak.db, frequencyHz: peak.frequencyHz ?? null };
}

function serialiseMetrics(metrics) {
  const out = {};
  if (metrics.payloadCrc !== undefined) out.payloadCrc = metrics.payloadCrc;
  if (metrics.preampDb !== undefined) out.preampDb = metrics.preampDb;
  if (metrics.quantizedPreampDb !== undefined) {
    out.quantizedPreampDb = metrics.quantizedPreampDb;
  }
  if (metrics.internalAudible !== undefined) {
    out.internalAudible = serialisePeak(metrics.internalAudible);
  }
  if (metrics.outputAudible !== undefined) out.outputAudible = serialisePeak(metrics.outputAudible);
  if (metrics.outputFull !== undefined) out.outputFull = serialisePeak(metrics.outputFull);
  if (metrics.internalFull !== undefined) out.internalFull = serialisePeak(metrics.internalFull);
  if (metrics.outputExtended !== undefined) {
    out.outputExtended = serialisePeak(metrics.outputExtended);
  }
  if (metrics.sections !== undefined) {
    out.sections = metrics.sections.map((section) => ({
      poleRadius: section.poleRadius,
      fullPeakDb: section.fullPeak.db,
    }));
  }
  if (metrics.prefixAudible !== undefined) {
    out.prefixAudible = metrics.prefixAudible.map((prefix) => ({
      stage: prefix.stage,
      db: prefix.db,
    }));
  }
  return out;
}

function serialiseResult(result) {
  return {
    ok: result.ok,
    reasons: result.reasons.map(serialiseReason),
    ...(result.waived !== undefined ? { waived: result.waived } : {}),
    ...(result.waivedReasons !== undefined
      ? { waivedReasons: result.waivedReasons.map(serialiseReason) }
      : {}),
  };
}

/** The frozen gate predates filters above 20 kHz, so it is only comparable on
 * curves that stay inside the audible band under the standard profile. */
function frozenComparable(bands, options) {
  if (options.profile === 'yolo') return false;
  return bands.every((band) => band.freqHz === undefined || band.freqHz <= 20000);
}

// --- corpus assembly --------------------------------------------------------

const cases = [];
const seen = new Set();
const coverage = new Map();

/**
 * Runs the gate over one curve and, unless `record` is false, keeps the verdict
 * in the corpus. Returns true when this case discovered a new reason code.
 */
function addCase({ id, bands, preampDb, options, note, record = true }) {
  const key = JSON.stringify([bands, preampDb, options]);
  if (seen.has(key)) return false;
  seen.add(key);

  const result = gate(bands, preampDb, options);
  const codes = [...result.reasons, ...(result.waivedReasons ?? [])].map(
    (reason) => reason.code,
  );
  const discovered = codes.filter((code) => !coverage.has(code));
  for (const code of discovered) coverage.set(code, id);

  if (!record && discovered.length === 0) return false;

  const frozen = frozenComparable(bands, options)
    ? (() => {
        try {
          return frozenGate(bands, preampDb, options);
        } catch (error) {
          return { threw: String(error?.message ?? error) };
        }
      })()
    : null;

  cases.push({
    id,
    note: note ?? null,
    bands,
    preampDb,
    options,
    expected: serialiseResult(result),
    metrics: serialiseMetrics(result.metrics),
    frozen: frozen && !frozen.threw ? serialiseResult(frozen) : null,
  });
  return discovered.length > 0;
}

// 1. Coverage sweep: a single non-OFF band among nine OFF keeps the scan analytic
//    (one active section), so this part is fast and pins every reason code.
//    `record: false` keeps only the cases that discover a new code, so the
//    ~24k combinations cost time but not corpus size.
const SWEEP_FREQS = [
  1, 2, 5, 10, 20, 50, 100, 500, 1000, 5000, 10000, 16000, 18000, 20000, 24000, 30000, 40000,
];
const SWEEP_Q = [0.1, 0.5, 1, 2, 4, 10, 50, 200, 1000, 10000];
const SWEEP_GAIN = [-24, -12, -6, 0, 3, 6, 9.2];
const SWEEP_KINDS = ['PK', 'LS', 'HS', 'LP', 'HP'];
const SWEEP_OPTIONS = [OPTIONS[0], OPTIONS[3]];
const SWEEP_PREAMPS = [-0.5, 0];

let sweepIndex = 0;
for (const kind of SWEEP_KINDS) {
  for (const freqHz of SWEEP_FREQS) {
    for (const qFactor of SWEEP_Q) {
      for (const gainDb of SWEEP_GAIN) {
        const band =
          kind === 'LP' || kind === 'HP'
            ? pass(kind, freqHz, qFactor)
            : kind === 'PK'
              ? pk(freqHz, gainDb, qFactor)
              : shelf(kind, freqHz, gainDb, qFactor);
        const bands = curve([band]);
        for (const options of SWEEP_OPTIONS) {
          for (const preampDb of SWEEP_PREAMPS) {
            addCase({
              id: `sweep-${sweepIndex++}`,
              bands,
              preampDb,
              options,
              note: 'coverage sweep',
              record: false,
            });
          }
        }
      }
    }
  }
}

// Structural codec refusals the sweep cannot produce (its bands are always valid).
addCase({
  id: 'codec-wrong-band-count',
  bands: NEUTRAL_BANDS.slice(0, 9),
  preampDb: -0.5,
  options: OPTIONS[0],
  note: 'nine bands instead of ten',
});
addCase({
  id: 'codec-zero-q',
  bands: curve([pk(1000, -6, 0)]),
  preampDb: -0.5,
  options: OPTIONS[0],
  note: 'qFactor 0 is refused by the codec',
});
addCase({
  id: 'codec-preamp-too-high',
  bands: [...NEUTRAL_BANDS],
  preampDb: 1,
  options: OPTIONS[0],
  note: 'a positive preamp is refused before it can clip',
});

// A handful of single-band curves under every option set, so the analytic
// single-section path is exercised on drag, engineering and yolo too.
for (const [index, band] of [
  pk(1000, -6, 1),
  pk(16000, 6, 4),
  pk(4000, 3, 10),
  shelf('HS', 8000, -3, 0.7),
  shelf('LS', 120, 6, 0.5),
  pass('LP', 6000, 0.707),
].entries()) {
  for (const [optionIndex, options] of OPTIONS.entries()) {
    addCase({
      id: `single-${index}-opt${optionIndex}`,
      bands: curve([band]),
      preampDb: -0.5,
      options,
      note: 'single active section',
    });
  }
}

// 2. The eleven official presets, at their own preamp and at three fixed ones.
for (const preset of OFFICIAL_PRESETS) {
  for (const preampDb of [preset.preampDb, -0.5, -12, -25.5]) {
    addCase({
      id: `preset-${preset.id}-${String(preampDb).replace('.', '_')}`,
      bands: preset.bands,
      preampDb,
      options: OPTIONS[0],
      note: preset.id,
    });
  }
}

// 3. Order sensitivity: the codec is order-independent, the cascade is not.
for (const preset of OFFICIAL_PRESETS) {
  addCase({
    id: `preset-${preset.id}-reversed`,
    bands: [...preset.bands].reverse(),
    preampDb: preset.preampDb,
    options: OPTIONS[0],
    note: `${preset.id} reversed`,
  });
  addCase({
    id: `preset-${preset.id}-sorted`,
    bands: [...preset.bands]
      .map((band, index) => ({ band, index }))
      .sort((a, b) => (a.band.freqHz ?? 0) - (b.band.freqHz ?? 0))
      .map(({ band }) => band),
    preampDb: preset.preampDb,
    options: OPTIONS[0],
    note: `${preset.id} sorted by frequency`,
  });
}

// 4. The neutral curve at the fixed preamps.
for (const preampDb of [-0.5, -12, -25.5]) {
  addCase({
    id: `neutral-${String(preampDb).replace('.', '_')}`,
    bands: [...NEUTRAL_BANDS],
    preampDb,
    options: OPTIONS[0],
    note: 'all bands OFF',
  });
}

// 5. Random curves, including a YOLO twin of each so both profiles see them.
const random = makeRandom(20261016);
for (let i = 0; i < 80; i++) {
  const bands = randomCurve(random);
  addCase({ id: `random-${i}`, bands, preampDb: -0.5, options: OPTIONS[0] });
  addCase({ id: `random-${i}-yolo`, bands, preampDb: -0.5, options: OPTIONS[3] });
}

// 6. Ultrasonic content, which only the current gate models (d7 above 20 kHz).
for (let i = 0; i < 15; i++) {
  const bands = randomUltrasonicCurve(random);
  addCase({ id: `ultrasonic-${i}`, bands, preampDb: -0.5, options: OPTIONS[0] });
  addCase({ id: `ultrasonic-${i}-yolo`, bands, preampDb: -0.5, options: OPTIONS[3] });
  addCase({ id: `ultrasonic-${i}-eng`, bands, preampDb: -0.5, options: OPTIONS[1] });
}

// 7. The presets again under YOLO, where waivers and the extended checks apply.
for (const preset of OFFICIAL_PRESETS) {
  addCase({
    id: `preset-${preset.id}-yolo`,
    bands: preset.bands,
    preampDb: preset.preampDb,
    options: OPTIONS[3],
    note: `${preset.id} under yolo`,
  });
  addCase({
    id: `preset-${preset.id}-yolo-boost`,
    bands: preset.bands.map((band) =>
      band.kind === 'PK' ? { ...band, gainDb: Math.abs(band.gainDb) + 3 } : band,
    ),
    preampDb: preset.preampDb,
    options: OPTIONS[3],
    note: `${preset.id} with every PK flipped to a boost`,
  });
}

// --- checks -----------------------------------------------------------------

const missing = ALL_CODES.filter((code) => !coverage.has(code));
if (missing.length > 0) {
  console.error(`coverage sweep missed ${missing.length} reason code(s): ${missing.join(', ')}`);
  process.exit(1);
}

await mkdir(dirname(OUT), { recursive: true });
await writeFile(
  OUT,
  `${JSON.stringify(
    {
      _provenance: {
        generatedBy: 'tools/gen-gate-verdicts.mjs',
        source: 'open-auri src/safety/gate.mjs',
        frozen: 'open-auri test/fixtures/gate-807745d.mjs',
        openAuri: OPEN_AURI,
      },
      codes: ALL_CODES,
      coverage: Object.fromEntries(coverage),
      cases,
    },
    null,
    2,
  )}\n`,
  'utf8',
);

console.log(`wrote ${cases.length} cases to ${OUT}`);
console.log(`covered ${coverage.size}/${ALL_CODES.length} reason codes`);
