import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan, unsigned, checkMarket} from '../src/lib/walletDeployment.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const read = path => readFileSync(new URL(path, import.meta.url));
const profiles = JSON.parse(read('../../config/first_run_dex.json'));
const ethereum = JSON.parse(read('../src/assets/carbon-ethereum.json'));
const base = JSON.parse(read('../src/assets/alien-carbon-base.json'));
const measured = JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/alien_carbon_base.json.gz')));
for (const [profile, plan, other] of [['carbon-defi-ethereum', ethereum, base], ['alien-carbon-base', base, ethereum]]) {
  test(profile+' deploys the controller selected for the venue chain', async () => {
    const settings = profiles.find(p => p.id === profile).settings;
    const row = {chainId:plan.chainId, kind:settings['dex.kind'], market:settings['dex.factory'],
      factoryHash:settings['dex.factory_runtime_hash'], implementations:settings['dex.proxy_implementations'],
      implementationHashes:settings['dex.proxy_runtime_hashes'], hashes:settings['dex.module_runtime_hashes']};
    assert.equal(await loadPlan(row,plan), plan);
    assert.equal(unsigned(plan).value, '0x0');
    assert.equal(BigInt(unsigned(plan).chainId),BigInt(plan.chainId));
    await assert.rejects(loadPlan(row,other));
    for (const change of [{chainId:other.chainId},{market:other.market},{hashes:[]},{implementations:[]},
      {implementationHashes:[]},{factoryHash:other.factoryHash}]) {
      await assert.rejects(loadPlan({...row,...change},plan));
    }
    await assert.rejects(loadPlan(row,{...plan,data:plan.data.slice(0,-2)+'00'}));
    if (plan === base) assert.deepEqual(plan,measured.protected.manifest);
  });
}

test('checks proxy, active implementation and both runtimes at the same block',async () => {
  const p = {...base,factoryHash:runtimeKeccak('0x11'),implementationHash:runtimeKeccak('0x22')};
  const at = {requireCanonical:true,blockHash:p.sourceBlock};
  let slot = '0x'+p.implementation.slice(2).padStart(64,'0');
  const code = {[p.market]:'0x11',[p.implementation]:'0x22'};
  const provider = {request:async ({method,params})=>{
    if (method==='eth_chainId') return '0x2105';
    assert.deepEqual(params.at(-1),at);
    if (method==='eth_getStorageAt') {
      assert.equal(params[0],p.market);
      assert.equal(params[1],'0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc');
      return slot;
    }
    assert.equal(method,'eth_getCode');
    return code[params[0]];
  }};
  await checkMarket(provider,p,at);
  code[p.implementation]='0x33';
  await assert.rejects(checkMarket(provider,p,at));
  code[p.implementation]='0x22';
  slot='0x'+ethereum.implementation.slice(2).padStart(64,'0');
  await assert.rejects(checkMarket(provider,p,at));
  await assert.rejects(checkMarket(provider,p,{...at,requireCanonical:false}));
});
