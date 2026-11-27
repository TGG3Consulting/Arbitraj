import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan, unsigned} from '../src/lib/walletDeployment.js';

const read = file => readFileSync(new URL(file,import.meta.url));
const plan = JSON.parse(read('../src/assets/yield-basis-ethereum.json'));
const presets = JSON.parse(read('../../config/first_run_dex.json'));
const archive = name => JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/'+name+'.json.gz')));

test('YieldBasis constructor and receipt refer to the measured Ethereum markets',async()=>{
  const p = presets.find(entry=>entry.id==='yield-basis-ethereum');
  const s = p.settings;
  const original = archive('yield_basis_ethereum');
  const wallet = archive('yield_basis_wallet');
  const deployment = {kind:s['dex.kind'],chainId:p.chain_id,market:s['dex.factory'],
    factoryHash:s['dex.factory_runtime_hash'],wrapped:plan.wrapped,hashes:s['dex.module_runtime_hashes']};
  assert.equal(original.factory,plan.factory);
  assert.equal(original.factory_code.keccak256,plan.factoryHash);
  assert.deepEqual(wallet.selector,original.selector);
  assert.equal(wallet.selector.blockHash,plan.sourceBlock);
  assert.equal(await loadPlan(deployment,plan),plan);
  assert.equal(unsigned(plan).value,'0x0');
  for (const change of [{kind:'balancer_v1'},{chainId:8453},{factoryHash:plan.wrappedHash},{hashes:[]},{wrapped:plan.factory}])
    await assert.rejects(loadPlan({...deployment,...change},plan));
  const versions = JSON.parse(read('../../tests/data/first_run_dex_2026-11-26/yield_basis_lev_versions.json'));
  for (const [venue, evidence] of Object.entries(original.venues)) {
    if (evidence.kind==='amm') {
      assert.equal(versions[venue].code,evidence.code.keccak256);
      assert.ok(s['dex.yield_basis_amm_hashes'].includes(versions[venue].code+' '+versions[venue].rule));
    } else {
      assert.ok(s['dex.yield_basis_pool_hashes'].includes(evidence.code.keccak256));
    }
    for (const sample of evidence.cases) {
      for (const mode of ['wallet','v5']) {
        const reproduced = wallet.cases.find(c=>c.mode===mode && c.venue===venue && c.side===sample.side && c.amount===sample.amount);
        assert.equal(reproduced.stage,sample.stage);
        assert.equal(reproduced.spent,sample.spent ?? '0');
        assert.equal(reproduced.received,sample.received ?? '0');
      }
    }
  }
});
