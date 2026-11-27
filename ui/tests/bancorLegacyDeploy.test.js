import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {loadPlan,unsigned} from '../src/lib/walletDeployment.js';

const read = path=>JSON.parse(readFileSync(new URL(path,import.meta.url)));
const plan=read('../src/assets/bancor-v21-ethereum.json');
const profile=read('../../config/first_run_dex.json').find(p=>p.id==='bancor-v21-ethereum').settings;
const settings={kind:profile['dex.kind'],chainId:1,wrapped:plan.wrapped,market:profile['dex.factory'],
  factoryHash:profile['dex.factory_runtime_hash'],quoter:profile['dex.quoter'],
  quoterHash:profile['dex.quoter_runtime_hash'],hashes:profile['dex.module_runtime_hashes']};
test('Bancor v2.1 deployment binds registry, trading network and wrapped ETH in order',async ()=>{
  assert.equal(await loadPlan(settings,plan),plan);
  assert.equal(unsigned(plan).value,'0x0');
  assert.equal(unsigned(plan).chainId,'0x1');
  for(const change of [{quoter:plan.factory},{quoterHash:plan.factoryHash},{kind:'bancor_v3'},{chainId:8453},{hashes:[]}]){
    await assert.rejects(loadPlan({...settings,...change},plan));
  }
  await assert.rejects(loadPlan(settings,{...plan, data:plan.data.slice(0,-64)+plan.network.slice(2).padStart(64,'0')}));
});
