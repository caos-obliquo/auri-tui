#!/usr/bin/env node
/**
 * Generates the `level_db` parity corpus.
 *
 * Runs the reference `levelDb` (the source of truth) over the 11 official
 * presets and the neutral curve, and writes the values to
 * `crates/auri-gate/tests/fixtures/level-values.json` for the Rust test
 * `crates/auri-gate/tests/level_parity.rs` to replay.
 *
 * The reference lives in the AGPL sibling repo and is never vendored here:
 * this script only *reads* it at generation time, and the generated JSON is
 * gitignored like every other oracle.
 *
 * Usage:
 *   node tools/gen-levels.mjs
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
const OUT = resolve(REPO_ROOT, 'crates/auri-gate/tests/fixtures/level-values.json');

const load = (relative) => import(pathToFileURL(resolve(OPEN_AURI, relative)).href);

const { levelDb } = await load('src/dsp/level.mjs');
const { OFFICIAL_PRESETS } = await load('src/catalog/presets.mjs');
const { NEUTRAL_BANDS } = await load('src/dsp/eq0xcb.mjs');

const cases = [];

// The 11 official presets at their own preamp.
for (const preset of OFFICIAL_PRESETS) {
  cases.push({
    id: `preset-${preset.id}`,
    bands: preset.bands,
    preampDb: preset.preampDb,
    levelDb: levelDb(preset.bands, preset.preampDb),
  });
}

// The neutral curve at several preamps, including the storable floor.
for (const preampDb of [-0.5, -12, -25.5]) {
  cases.push({
    id: `neutral-${String(preampDb).replace('.', '_')}`,
    bands: [...NEUTRAL_BANDS],
    preampDb,
    levelDb: levelDb(NEUTRAL_BANDS, preampDb),
  });
}

await mkdir(dirname(OUT), { recursive: true });
await writeFile(
  OUT,
  `${JSON.stringify(
    {
      _provenance: {
        generatedBy: 'tools/gen-levels.mjs',
        source: 'open-auri src/dsp/level.mjs',
        openAuri: OPEN_AURI,
      },
      cases,
    },
    null,
    2,
  )}\n`,
  'utf8',
);

console.log(`wrote ${cases.length} cases to ${OUT}`);
