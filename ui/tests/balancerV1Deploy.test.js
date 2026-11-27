import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan, checkMarket, unsigned} from '../src/lib/walletDeployment.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const read = p => readFileSync(new URL(p,import.meta.url));
const plan = JSON.parse(read('../src/assets/balancer-v1-ethereum.json'));
const archive = name => JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/'+name+'.json.gz')));
const direct = archive('balancer_v1_ethereum');
const wallet = archive('balancer_v1_wallet');
const profile = JSON.parse(read('../../config/first_run_dex.json')).find(x => x.id === 'balancer-v1-ethereum');
const s = profile.settings;
const settings = {kind:s['dex.kind'], chainId:profile.chain_id, market:s['dex.factory'],
  factoryHash:s['dex.factory_runtime_hash'], hashes:s['dex.module_runtime_hashes'], wrapped:plan.wrapped};

test('Balancer V1 wallet creation matches the original factory, wrapper and every archived cash mode',async()=>{
  assert.equal(plan.factory,direct.factory);
  assert.equal(plan.factoryHash,direct.factory_code.keccak256);
  assert.equal(plan.runtimeKeccak,wallet.module_hash);
  assert.equal(plan.sourceBlock,wallet.selector.blockHash);
  assert.equal(await loadPlan(settings,plan),plan);
  assert.deepEqual(unsigned(plan),{chainId:'0x1',value:'0x0',data:plan.data});
  for (const change of [{chainId:8453},{wrapped:plan.factory},{market:plan.wrapped},{hashes:[]}]) {
    await assert.rejects(loadPlan({...settings,...change},plan));
  }
  await assert.rejects(loadPlan(settings,{...plan,data:plan.data.slice(0,-2)+'00'}));
});

test('BFactory and WETH deployed code must match on the same canonical block',async()=>{
  const at = {requireCanonical:true,blockHash:plan.sourceBlock};
  const codes = {[plan.factory]:'0x11',[plan.wrapped]:'0x22'};
  const p = {...plan,factoryHash:runtimeKeccak('0x11'),wrappedHash:runtimeKeccak('0x22')};
  const provider = {request:async ({method,params})=>{
    if (method === 'eth_chainId') return '0x1';
    assert.equal(method,'eth_getCode');
    assert.deepEqual(params[1],at);
    return codes[params[0]];
  }};
  await checkMarket(provider,p,at);
  codes[plan.wrapped]='0x33';
  await assert.rejects(checkMarket(provider,p,at),/WETH/);
  await assert.rejects(checkMarket(provider,p,{...at,requireCanonical:false}),/канонический/);
});
