import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {createHash} from 'node:crypto';
import {loadPlan} from '../src/lib/walletDeployment.js';
import {addZeroEx,checkZeroExModule,registeredZeroEx} from '../src/lib/zeroexDeploy.js';
import sha3 from 'js-sha3';

const load=(p)=>readFileSync(new URL(p,import.meta.url));
const card=JSON.parse(load('../src/assets/zeroex-ethereum-create.json'));
const witness=JSON.parse(gunzipSync(load('../../tests/data/first_run_dex_2026-11-26/zeroex_ethereum_wallet.json.gz')));
const wallet=witness.payer;
const settings={kind:'zeroex',chainId:1,wallet,rows:null};
const roles=['holder','holderHash','registry','registryHash'];
test('0x Ethereum CREATE matches the paid module and chain',async()=>{
  const plan=await loadPlan(settings,card);
  assert.equal(plan.data,witness.creation);
  assert.equal(plan.runtimeSha256,createHash('sha256').update(Buffer.from(witness.runtime.slice(2),'hex')).digest('hex'));
  assert.equal(plan.wallet,wallet.toLowerCase());
  assert.equal(addZeroEx(null,plan,witness.module),witness.module);
  assert.throws(()=>addZeroEx(card.registry,plan,witness.module));
  await assert.rejects(loadPlan({...settings,chainId:8453},card));
  await assert.rejects(loadPlan(settings,{...card,holderHash:card.registryHash}));
  await assert.rejects(loadPlan(settings,{...card,data:card.data+'00'}));
});
test('all four constructor getters must agree on the same canonical block',async()=>{
  const observed=new Set();
  const mock={request:async({method,params})=>{
    assert.equal(method,'eth_call');
    assert.deepEqual(params[1],witness.selector);
    assert.equal(params[0].to,witness.module);
    const k=roles.find(k=>params[0].data==='0x'+sha3.keccak_256(k+'()').slice(0,8));
    observed.add(k);
    return '0x'+card[k].slice(2).padStart(64,'0');
  }};
  await checkZeroExModule(mock,card,witness.module,witness.selector);
  assert.equal(observed.size,4);
  await assert.rejects(checkZeroExModule({request:async()=>card.holderHash},card,witness.module,witness.selector));
  assert.equal(await registeredZeroEx(mock,card,null),null);
});
