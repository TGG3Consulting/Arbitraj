// Платные CREATE не отправляются. Карточки и байты двух runtime записаны GitHub replay.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import test from 'node:test';
import {loadPlan, unsigned} from '../src/lib/walletDeployment.js';
import {recoverDexalot, registeredDexalotVersions} from '../src/lib/dexalotVersions.js';

const asset = name => JSON.parse(readFileSync(new URL('../src/assets/'+name,import.meta.url)));
const archive = name => JSON.parse(gunzipSync(readFileSync(new URL(
  '../../tests/data/first_run_dex_2026-11-26/'+name,import.meta.url))));
const current=asset('dexalot-base-create.json'), previous=asset('dexalot-base-create-legacy.json');
const {owner,fixture_module:target,head}=archive('dexalot_base_wallet.json.gz');
const {previousRuntime,currentRuntime}=archive('dexalot_creation_v2.json.gz');
const settings={kind:current.kind,chainId:current.chainId,wallet:owner,rows:Object.values(current.roles).map(
  r=>[r.address===current.router?'router':'maker',r.address,r.hash,
    ...(r.implementation?[r.implementation.address,r.implementation.hash]:[])].join(' '))};
const legacy=async()=>previous;
const word=a=>'0x'+a.slice(2).padStart(64,'0');

test('новый CREATE использует действующую карточку, для старого поручения восстанавливается точная исходная',async()=>{
  const plan=await loadPlan(settings,current);
  assert.equal(unsigned(plan).data,current.data);
  assert.notEqual(previous.creationSha256,current.creationSha256);
  const memo={from:owner,chain:current.chainId,creation:previous.creationSha256};
  const rpc={request:()=>assert.fail('Для выбора карточки сохранённого поручения RPC не нужен')};
  assert.equal((await recoverDexalot(rpc,settings,plan,memo,null,legacy)).data,previous.data);
  await assert.rejects(recoverDexalot(rpc,settings,plan,{...memo,from:target},null,legacy),/кошельку/);
  await assert.rejects(recoverDexalot(rpc,settings,plan,{...memo,chain:1},null,legacy),/сети/);
  await assert.rejects(recoverDexalot(rpc,settings,plan,{...memo,creation:previous.runtimeSha256},null,legacy),/CREATE/);
  const lookup={request:({method})=>{
    assert.equal(method,'eth_getTransactionByHash'); return {input:previous.data};
  }};
  assert.equal((await recoverDexalot(lookup,settings,plan,null,head.blockHash,legacy)).runtimeKeccak,previous.runtimeKeccak);
  await assert.rejects(recoverDexalot({request:()=>({input:previousRuntime})},settings,plan,null,head.blockHash,legacy),/CREATE/);
});

test('каждый принятый runtime привязан к своему хешу, сети и владельцу на одном блоке',async()=>{
  const plan=await loadPlan(settings,current);
  const node=(code,who=owner,chain=current.chainId)=>({request:({method,params})=>{
    switch(method){
      case 'eth_chainId': return chain;
      case 'eth_getBlockByNumber': return {hash:head.blockHash};
      case 'eth_getCode': assert.deepEqual(params,[target,head]); return code;
      case 'eth_call': assert.deepEqual(params,[{to:target,data:'0x8da5cb5b'},head]); return word(who);
      default: assert.fail(method);
    }
  }});
  for (const [card,code] of [[previous,previousRuntime],[current,currentRuntime]]) {
    const rows=[...settings.rows,'module '+target+' '+card.runtimeKeccak];
    assert.equal(await registeredDexalotVersions(node(code),plan,rows,legacy),target);
    assert.equal(await registeredDexalotVersions(node(code,target),plan,rows,legacy),null);
    assert.equal(await registeredDexalotVersions(node(code===previousRuntime?currentRuntime:previousRuntime),plan,rows,legacy),null);
    await assert.rejects(registeredDexalotVersions(node(code,owner,1),plan,rows,legacy),/сеть/);
  }
});
