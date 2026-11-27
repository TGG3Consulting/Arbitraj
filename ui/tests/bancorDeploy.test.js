import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {checkMarket, loadPlan, unsigned} from '../src/lib/walletDeployment.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const read = path => readFileSync(new URL(path, import.meta.url));
const plan = JSON.parse(read('../src/assets/bancor-v3-ethereum.json'));
const original = JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/bancor_v3_ethereum.json.gz')));
const replay = JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/bancor_v3_wallet.json.gz')));
const catalog = JSON.parse(read('../../config/first_run_dex.json')).find(p => p.id === 'bancor-v3-ethereum');
const settings = catalog.settings;
const row = {kind:settings['dex.kind'],chainId:catalog.chain_id,market:settings['dex.factory'],
  quoter:settings['dex.quoter'],vault:settings['dex.vault'],native:settings['dex.bancor_native_token'],
  roles:settings['dex.bancor_roles'],hashes:settings['dex.module_runtime_hashes'],wrapped:plan.arguments[3]};

test('Bancor CREATE binds all five constructor arguments and the five deployed proxy implementations', async () => {
  assert.deepEqual(plan.roles, original.roles);
  assert.equal(plan.sourceBlock, original.selector.blockHash);
  assert.equal(plan.runtimeKeccak, replay.module_hash);
  assert.deepEqual(await loadPlan(row, plan), plan);
  assert.deepEqual(unsigned(plan), {chainId:'0x1',data:plan.data,value:'0x0'});
  for (const [name, value] of [['chainId',8453],['market',row.quoter],['vault',row.native],
    ['roles',row.roles.slice(1)],['hashes',[]],['native',row.wrapped]]) {
    await assert.rejects(loadPlan({...row,[name]:value},plan));
  }
  await assert.rejects(loadPlan(row,{...plan,data:plan.data.slice(0,-2)+'00'}));
});

test('Bancor checks active implementations and the native sentinel at the same canonical block', async () => {
  const roles = structuredClone(plan.roles);
  const at = {blockHash:plan.sourceBlock,requireCanonical:true};
  const code = new Map([[row.native,'0x'],[row.wrapped,'0xab']]);
  const slots = new Map();
  for (const [i,r] of Object.values(roles).entries()) {
    const runtime = '0x' + (10+i).toString(16).padStart(2,'0');
    code.set(r.address, '0x42');
    code.set(r.implementation.address, runtime);
    r.keccak256 = runtimeKeccak('0x42');
    r.implementation.keccak256 = runtimeKeccak(runtime);
    slots.set(r.address,'0x'+r.implementation.address.slice(2).padStart(64,'0'));
  }
  const provider = {request:async ({method,params}) => {
    if (method === 'eth_chainId') return '0x1';
    assert.deepEqual(params.at(-1),at);
    if (method === 'eth_getCode') return code.get(params[0]);
    assert.equal(method,'eth_getStorageAt');
    assert.equal(params[1], '0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc');
    return slots.get(params[0]);
  }};
  const p = {...plan,roles,wrappedHash:runtimeKeccak('0xab')};
  await checkMarket(provider,p,at);
  for (const role of Object.values(roles)) {
    const saved = slots.get(role.address);
    slots.set(role.address,'0x'+'00'.repeat(32));
    await assert.rejects(checkMarket(provider,p,at),/реализация/);
    slots.set(role.address,saved);
  }
  code.set(row.native,'0x01');
  await assert.rejects(checkMarket(provider,p,at),/нативного/);
});
