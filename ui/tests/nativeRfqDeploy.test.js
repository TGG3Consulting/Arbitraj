// Р”Р°РЅРЅС‹Рµ РїРѕР»СѓС‡РµРЅС‹ РёР· РёР·РѕР»РёСЂРѕРІР°РЅРЅРѕРіРѕ CREATE Рё 16 СЃРёРјСѓР»СЏС†РёР№ Native RFQ РІ GitHub Actions 46096868177.
import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan,sameRuntime,unsigned} from '../src/lib/walletDeployment.js';
import {baseNetwork} from '../src/lib/baseNetwork.js';
import {deploymentScope,sourceDeploymentKey} from '../src/lib/deploymentScope.js';
import {addNative,checkNativeModule,nativeRfqNetwork,checkNativeRoles} from '../src/lib/nativeRfqDeploy.js';

const read=path=>readFileSync(new URL(path,import.meta.url));
const card=JSON.parse(read('../src/assets/native-rfq-base-create.json'));
const archive=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/native_rfq_base_wallets.json.gz')));
const roles=JSON.parse(read('../../tests/data/first_run_dex_2026-11-26/native_rfq_base_roles.json')).roles;
const rows=['network 8453 base',...Object.entries(roles).map(([k,v])=>[k,v.address,v.runtime].join(' '))];
const args={kind:card.kind,chainId:8453,wallet:archive.payer,rows};
const target=archive.observed.find(r=>r.mode==='module').module;

test('Native Base pins both protocol roles, its compiled module and selected chain',async()=>{
  const p=await loadPlan(args,card);
  assert.equal(p.sourceBlock,archive.block_hash);
  assert.equal(await sameRuntime(archive.runtime,p),true);
  assert.equal(unsigned(p).data,p.data);
  assert.equal(p.runtimeKeccak,archive.observed.find(r=>r.mode==='module').runtime);
  assert.equal(baseNetwork('second',[{venue:'base_dex',chain:'base',evm_chain_id:8453}],
    {'dex.chain|Venue|second':'base','dex.evm_chain_id|Venue|second':8453}),8453);
  await assert.rejects(loadPlan({...args,chainId:1},card));
  await assert.rejects(loadPlan({...args,rows:[rows[0],rows[1],rows[1]]},card));
  await assert.rejects(loadPlan({...args,rows:[rows[0],rows[1],rows[2].replace(card.poolHash,card.routerHash)]},card));
  await assert.rejects(checkNativeModule({request:async()=>card.routerHash},p,target,{blockHash:p.sourceBlock,requireCanonical:true}));
});

test('Native keeps the original role list on reload and installs one module for the entire venue',async()=>{
  const p=await loadPlan(args,card), next=addNative(rows,p,target);
  assert.deepEqual(next,[...rows,'module '+target+' '+p.runtimeKeccak]);
  assert.equal(addNative(next,p,target),next);
  assert.throws(()=>addNative(next,p,roles.router.address));
  const key=sourceDeploymentKey('native-rfq','base_dex',8453,'6',p.wallet,true);
  assert.equal(key,sourceDeploymentKey('native-rfq','base_dex',8453,'6',target,true));
  const values=new Map(),storage={getItem:k=>values.get(k),setItem:(k,v)=>values.set(k,v)};
  let selected={id:p.wallet,kind:p.kind,venue:'base_dex',key,settings:{...args},router:null};
  const scope=()=>deploymentScope(storage,()=>selected);
  scope().save({chain:8453,creation:p.creationSha256,from:p.wallet,nonce:'0x0'});
  const reopened=scope();
  assert.equal(reopened.read().creation,p.creationSha256);
  let saved;
  await reopened.installSource('native_rfq.roles',next,(...v)=>{saved=v;});
  assert.deepEqual(saved,['native_rfq.roles',{kind:'Venue',id:'base_dex'},next,rows]);
  selected={...selected,settings:{...args,rows:next}};
  await assert.rejects(reopened.installSource('native_rfq.roles',next,()=>assert.fail()));
});

test('Ethereum uses its archived registry, immutable runtime and own signing account',async()=>{
  const eth=JSON.parse(read('../src/assets/native-rfq-ethereum-create.json'));
  const proof=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_create.json.gz')));
  const receipts=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_protected.json.gz')));
  const roles=JSON.parse(read('../../tests/data/first_run_dex_2026-11-26/native_rfq_ethereum_public.json')).roles.roles;
  const ethRows=['network 1 ethereum',...roles.map((r,i)=>[i?'pool':'router',r.address,r.runtime].join(' '))];
  const settings={kind:eth.kind,chainId:1,wallet:receipts.payer,rows:ethRows};
  const p=await loadPlan(settings,eth);
  assert.equal(await sameRuntime(proof.runtime,p),true);
  assert.equal(unsigned(p).chainId,'0x1');
  assert.ok(receipts.observed.length===12 && receipts.observed.every(r=>r.runtime===p.runtimeKeccak));
  assert.equal(addNative(ethRows,p,receipts.observed[0].module).length,4);
  await assert.rejects(loadPlan({...settings,rows},eth));
  const profile=[{venue:'eth_dex',evm_chain_id:1,chain:'ethereum'}];
  const selected={'dex.chain|Venue|other':'ethereum','dex.evm_chain_id|Venue|other':1};
  assert.equal(nativeRfqNetwork('other',profile,selected),1);
  assert.equal(nativeRfqNetwork('other',profile,{...selected,'dex.evm_chain_id|Venue|other':56}),null);
  const [call,at]=proof.registry.request;
  assert.equal(call.to,p.router);
  assert.equal(at.blockHash,p.sourceBlock);
  await assert.rejects(checkNativeRoles({request:async()=>proof.runtime},p,at));
});
