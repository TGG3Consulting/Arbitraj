import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { checkedRows, coinRowKey } from '../src/lib/coinFilter.js';

test('one address on two networks keeps separate rows, including an untagged DEX event', () => {
  const source = new URL('../../tests/data/mexc_2026-11-21/config_getall.json', import.meta.url);
  const mlk = JSON.parse(readFileSync(source, 'utf8')).find((c) => c.coin === 'MLK');
  const contract = (network) => mlk.networkList.find((n) => n.netWork === network).contract;
  assert.equal(contract('BSC'), contract('ARB'));
  const a = { asset: { ticker: 'MLK', bsc_contract: contract('BSC') }, presence: [{ venue: 'bsc_dex' }] };
  const b = { asset: { ...a.asset, chain: 'arbitrum' }, presence: [{ venue: 'arb_dex' }] };
  assert.notEqual(coinRowKey(a), coinRowKey(b));
  const rows = [a, b];
  const raw = { type: 'pool_check', data: { asset: a.asset, venue: 'arb_dex', pool: {} } };
  assert.deepEqual(checkedRows(rows, raw), [b]);
  assert.deepEqual(checkedRows(rows, { type: 'coin_block', data: { asset: b.asset } }), [b]);
  assert.deepEqual(checkedRows(rows, { type: 'coin_block', data: { asset: a.asset } }), [a]);
  assert.deepEqual(checkedRows([a], raw), []);
  const oldPresence = { ...a, dex_venues: ['bsc_dex'], presence: [{ venue: 'arb_dex' }] };
  const discovered = { ...b, dex_venues: ['arb_dex'], presence: [] };
  assert.deepEqual(checkedRows([oldPresence, discovered], raw), [discovered]);
});
