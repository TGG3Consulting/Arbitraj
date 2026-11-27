import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {priceBuilder} from '../src/lib/sourcePriceBuilder.js';

const record = JSON.parse(readFileSync(new URL('../../tests/data/first_run_dex_2026-11-26/prop_ethereum_builder_frames.json', import.meta.url)));
test('renamed Lambda uses only its configured builders after settings change', () => {
  const markets = new Map();
  for (const frame of record.frames) {
    const builders = markets.get(frame.name) ?? [];
    builders.push(frame.builder.toLowerCase());
    markets.set(frame.name, builders);
  }
  const source = 'lambda:dexalot';
  const titanOnly = markets.get('kaliber');
  const two = markets.get('taurus');
  assert.deepEqual(priceBuilder(source, titanOnly, 'buildernet'), {required:true, choices:['titan'], selected:'titan'});
  assert.equal(priceBuilder(source, two, 'buildernet').selected, 'buildernet');
  assert.equal(priceBuilder(source, [...two].reverse(), '').selected, 'buildernet');
  assert.deepEqual(priceBuilder(source, [], 'titan'), {required:true, choices:[], selected:undefined});
  assert.equal(priceBuilder('bopamm', null, '').selected, 'buildernet');
  assert.equal(priceBuilder('dexalot', null, '').required, false);
});
