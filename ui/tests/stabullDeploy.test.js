import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan, unsigned, checkMarket} from '../src/lib/walletDeployment.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const read = path => readFileSync(new URL(path, import.meta.url));
const plan = JSON.parse(read('../src/assets/stabull-base.json'));
const replay = JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/stabull_base_protected_wallets.json.gz')));
const catalog = JSON.parse(read('../../config/first_run_dex.json')).find(row => row.id === 'stabull');
const row = {chainId:catalog.chain_id,kind:catalog.settings['dex.kind'],market:catalog.settings['dex.factory'],
  factoryHash:catalog.settings['dex.factory_runtime_hash'],hashes:catalog.settings['dex.module_runtime_hashes']};

test('Stabull binds the archived unsigned CREATE and sole constructor role to Base settings', async () => {
  assert.equal(plan.runtimeKeccak, replay.module.runtime);
  assert.equal(plan.archiveGas, replay.module.deployment_gas);
  assert.equal(plan.blockHash, replay.selector.blockHash);
  assert.deepEqual(unsigned(plan), {chainId:'0x2105',data:plan.data,value:'0x0'});
  for (const kind of ['stabull','stabull_module']) assert.deepEqual(await loadPlan({...row,kind},plan),plan);
  for (const change of [{chainId:1},{kind:'woofi'},{market:'0x'+'13'.repeat(20)},{hashes:[]},{factoryHash:plan.runtimeKeccak}]) {
    await assert.rejects(loadPlan({...row,...change},plan));
  }
  await assert.rejects(loadPlan(row,{...plan,data:plan.data.slice(0,-2)+'00'}));
});

test('Stabull checks the factory code at the exact canonical block and verifies the chain', async () => {
  const at = {blockHash:plan.blockHash,requireCanonical:true};
  const code = '0x6001';
  const provider = {request:async ({method,params}) => {
    if (method === 'eth_chainId') return '0x2105';
    assert.equal(method,'eth_getCode');
    assert.deepEqual(params,[row.market,at]);
    return code;
  }};
  await checkMarket(provider,{...plan,factoryHash:runtimeKeccak(code)},at);
  await assert.rejects(checkMarket(provider,plan,at),/фабрики/);
  await assert.rejects(checkMarket(provider,plan,{...at,requireCanonical:false}),/канонический/);
  await assert.rejects(checkMarket({request:async()=> '0x1'},plan,at),/Base/);
});

test('Ethereum Stabull binds its own paid factory and refuses the Base wallet settings', async () => {
  const eth = JSON.parse(read('../src/assets/stabull-ethereum.json'));
  const replay = JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/stabull_ethereum_protected_wallets.json.gz')));
  const profile = JSON.parse(read('../../config/first_run_dex.json')).find(p => p.id === 'stabull-ethereum');
  const cfg = {chainId:profile.chain_id,kind:profile.settings['dex.kind'],market:profile.settings['dex.factory'],
    factoryHash:profile.settings['dex.factory_runtime_hash'],hashes:profile.settings['dex.module_runtime_hashes']};
  assert.equal(eth.runtimeKeccak, replay.module.runtime);
  assert.equal(eth.market, replay.module.factory);
  assert.equal(eth.archiveGas, replay.module.deployment_gas);
  assert.equal(eth.blockHash, replay.selector.blockHash);
  assert.deepEqual(unsigned(eth), {chainId:'0x1',data:eth.data,value:'0x0'});
  for (const kind of ['stabull','stabull_module']) assert.equal(await loadPlan({...cfg,kind},eth),eth);
  await assert.rejects(loadPlan(row,eth));
  await assert.rejects(loadPlan(cfg,plan));
});
