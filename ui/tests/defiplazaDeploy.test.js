import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadPlan, checkMarket, unsigned} from '../src/lib/walletDeployment.js';

const read = file => JSON.parse(readFileSync(new URL(file, import.meta.url)));
const profiles = read('../../config/first_run_dex.json');
for (const name of ['multi','stable']) {
  test('DefiPlaza binds the generation, vault, WETH and V5 caller for '+name, async () => {
    const plan = read('../src/assets/defiplaza-'+name+'-create.json');
    const s = profiles.find(p=>p.id==='defiplaza-'+name+'-ethereum').settings;
    const settings = {kind:s['dex.kind'],chainId:1,market:s['dex.factory'],factoryHash:s['dex.factory_runtime_hash'],
      wrapped:plan.roles.wrapped.address,hashes:s['dex.module_runtime_hashes']};
    assert.equal(await loadPlan(settings,plan),plan);
    assert.equal(unsigned(plan).data,plan.data);
    await assert.rejects(loadPlan({...settings,chainId:56},plan));
    await assert.rejects(loadPlan({...settings,wrapped:settings.market},plan));
    await assert.rejects(loadPlan(settings,{...plan,stable:!plan.stable}));
    await assert.rejects(loadPlan(settings,{...plan,executorHash:plan.roles.factory.keccak256}));
    await assert.rejects(loadPlan({...settings,hashes:[]},plan));
    const at = {blockHash:plan.sourceBlock, requireCanonical:true};
    const calls = [];
    const provider = {request:async({method,params})=>{
      calls.push([method,params]);
      return method==='eth_chainId'?'0x1':'0x00';
    }};
    await assert.rejects(checkMarket(provider,plan,at),/код контракта/);
    assert.deepEqual(calls.filter(([m])=>m==='eth_getCode').map(([,args])=>args),
      [[settings.market,at],[settings.wrapped,at]]);
  });
}
