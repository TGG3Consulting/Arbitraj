import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {selectPropDeployment} from '../src/lib/propVenueDeploy.js';
import {deployed, hashCode, loadPlan, sameRuntime} from '../src/lib/walletDeployment.js';

const read = path => readFileSync(new URL(path, import.meta.url));
const cards = ['prop-slot-v2-ethereum', 'prop-slot-v2-ethereum-previous',
  'prop-slot-venue-ethereum', 'prop-venue-ethereum']
  .map(name => JSON.parse(read('../src/assets/'+name+'.json')));
const [current, previous, firstSlot, ordinary] = cards;
const profiles = JSON.parse(read('../../config/first_run_dex.json'));
const recorded = file => JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/'+file+'.json.gz')));
const evidence = recorded('prop_slot_creation_v2');
const profile = market => {
  const p = profiles.find(p => p.chain_id===1 && p.settings['dex.kind']==='lambda_propamm'
    && p.settings['dex.factory']===market);
  const s = p.settings;
  return {chainId:p.chain_id, kind:s['dex.kind'], market, quoter:s['dex.quoter'],
    propRoles:s['dex.propamm_roles'], hashes:s['dex.module_runtime_hashes']};
};
const choose = (s, pending, input) => selectPropDeployment(s,pending,input,cards);

test('each market keeps its own allowed CREATE generations, including explicit old-only profiles',async()=>{
  for (const market of Object.keys(ordinary.markets)) {
    const settings = profile(market);
    const applicable = cards.filter(c => c.markets[market]);
    assert.equal(choose(settings), applicable[0]);
    assert.throws(()=>choose(settings,{chain:1,creation:"unknown"}));
    assert.equal(new Set(applicable.map(c => c.markets[market].creationSha256)).size, applicable.length);
    for (const card of applicable) {
      const p = await loadPlan(settings,card);
      const intent = {chain:p.chainId, creation:p.creationSha256};
      assert.equal(choose(settings,intent),card);
      assert.equal(choose(settings,null,p.data),card);
      assert.equal(choose({...settings,hashes:[p.runtimeKeccak]}),card);
      assert.throws(()=>choose({...settings,hashes:[]},intent));
      assert.throws(()=>choose({...settings,chainId:8453},intent));
      assert.throws(()=>choose(settings,{...intent,chain:8453}));
      assert.throws(()=>choose(settings,null,p.data+'00'));
      const foreign = Object.keys(card.markets).find(address=>address!==market);
      assert.throws(()=>choose(settings,{...intent,creation:card.markets[foreign].creationSha256}));
    }
  }
});

test('both funded v2 archives and the rebuilt constructor match exactly one allowed deployed runtime',async()=>{
  assert.equal(current.sourceBlock,evidence.verification.sourceBlock);
  for (const name of ['kaliber','taurus']) {
    const archive = recorded(name+'_ethereum_bound_v2');
    const market = archive.builders[0].market;
    const settings = profile(market);
    const former = await loadPlan(settings,previous);
    const active = await loadPlan(settings,current);
    const {previousRuntime,currentRuntime} = evidence.markets[market];
    const metrics = evidence.verification.markets[market];
    const prefixLength = 2 + 2*metrics.identicalRuntimeBytes;
    assert.ok(metrics.identicalRuntimeBytes > 0 && prefixLength < currentRuntime.length);
    assert.equal(previousRuntime.slice(0,prefixLength),currentRuntime.slice(0,prefixLength));
    assert.equal(await hashCode(currentRuntime.slice(0,prefixLength)), metrics.runtimeProgramSha256);
    assert.equal(metrics.previousRuntime,former.runtimeKeccak);
    assert.equal(metrics.runtime,active.runtimeKeccak);
    assert.deepEqual([...new Set(archive.builders.map(b=>b.runtime))],[former.runtimeKeccak]);
    assert.notEqual(former.creationSha256,active.creationSha256);
    assert.notEqual(former.runtimeKeccak,active.runtimeKeccak);
    assert.equal(await sameRuntime(previousRuntime,former),true);
    assert.equal(await sameRuntime(currentRuntime,active),true);
    assert.equal(await sameRuntime(currentRuntime,former),false);
    assert.equal(await sameRuntime(previousRuntime,active),false);
    const oldSlot = recorded(name+'_ethereum_bound_wallet');
    assert.deepEqual([...new Set(oldSlot.builders.map(b=>b.runtime))],[firstSlot.markets[market].runtimeKeccak]);
  }
});

test('an original pending CREATE is reconciled with its recorded wallet, nonce, chain and initcode',async()=>{
  const row = recorded('kaliber_ethereum_wallets');
  const market = Object.keys(previous.markets)[0];
  const settings = profile(market);
  const former = await loadPlan(settings,previous);
  const pending = {chain:1, creation:former.creationSha256, from:row.owner.toLowerCase(), nonce:'0x7'};
  const plan = await loadPlan(settings,choose(settings,pending));
  const hash = '0x'+'ab'.repeat(32);
  const blockHash = former.sourceBlock;
  const tx = {hash, blockHash, from:pending.from, nonce:pending.nonce, to:null, value:'0x0', input:plan.data, chainId:'0x1'};
  const receipt = {transactionHash:hash, blockNumber:'0x20', blockHash, status:'0x0'};
  const provider = {request:async({method})=>{
    switch (method) {
      case 'eth_chainId': return '0x1';
      case 'eth_requestAccounts': return [pending.from];
      case 'eth_getTransactionByHash': return tx;
      case 'eth_getTransactionReceipt': return receipt;
      case 'eth_getBlockByNumber': return {hash:blockHash, number:receipt.blockNumber, transactions:[hash]};
      default: throw Error('Unexpected method '+method);
    }
  }};
  assert.deepEqual(await deployed(provider,plan,hash,pending),{reverted:true});
  const manual = await loadPlan(settings,choose(settings,null,tx.input));
  assert.deepEqual(await deployed(provider,manual,hash,null),{reverted:true});
  await assert.rejects(deployed(provider,plan,hash,{...pending,nonce:'0x8'}));
  await assert.rejects(deployed(provider,plan,hash,{...pending,from:market}));
  tx.input='0x';
  const consumed=await deployed(provider,plan,hash,pending);
  assert.equal(consumed.consumed,true);
  assert.equal(consumed.earlierCreation,false);
  assert.equal(consumed.address,undefined);
});
