import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import sha3 from 'js-sha3';
import {deploymentScope} from '../src/lib/deploymentScope.js';
import {loadPlan} from '../src/lib/walletDeployment.js';
import {checkAmbient} from '../src/lib/ambientDeploy.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const p=JSON.parse(readFileSync(new URL('../src/assets/ambient-ethereum-create.json',import.meta.url)));
const profiles=JSON.parse(readFileSync(new URL('../../config/first_run_dex.json',import.meta.url)));
const row=profiles.find(v=>v.chain_id===1 && v.id==='ambient-ethereum').settings;
const s={kind:row['dex.kind'],chainId:1,market:row['dex.factory'],ambientQuery:row['dex.ambient_query'],quoter:row['dex.quoter'],
         wrapped:p.wrapped,hashes:row['dex.module_runtime_hashes'],ambientRoles:row['dex.ambient_roles']};

test('Ambient CREATE uses the registered chain, venues, wrapped asset and guarded runtime',async()=>{
  assert.equal(await loadPlan(s,p),p);
  for(const change of [{chainId:8453},{ambientQuery:s.market},{wrapped:s.quoter},{hashes:[]},{ambientRoles:s.ambientRoles.slice(1)},
                       {ambientRoles:[...s.ambientRoles,s.ambientRoles[0]]},
                       {ambientRoles:[...s.ambientRoles,'other '+p.exchange+' '+p.runtimeKeccak]},
                       {ambientRoles:s.ambientRoles.map(r=>r.replace(/^dex /,'DEX '))},
                       {hashes:[...s.hashes,p.runtimeKeccak]}])
    await assert.rejects(loadPlan({...s,...change},p));
  await assert.rejects(loadPlan(s,{...p,data:p.data+'00'}));
});

test('Canonical read detects a replaced knockout callpath even when the singleton code stays identical',async()=>{
  const code='0x6000',roles=Object.fromEntries(Object.entries(p.roles).map(([n,r])=>[n,{...r,hash:runtimeKeccak(code)}]));
  const plan={...p,roles}, at={blockHash:p.sourceBlock,requireCanonical:true};
  const readSlot='0x'+sha3.keccak_256('readSlot(uint256)').slice(0,8);
  function node(replaced){
    return {request:async({method,params})=>{
      assert.deepEqual(params[1],at);
      if(method==='eth_getCode') return code;
      assert.equal(method,'eth_call');
      const {to,data}=params[0];
      let address=p.exchange;
      if(to===p.exchange){
        assert.ok(data.startsWith(readSlot));
        const slot=BigInt('0x'+data.slice(10));
        assert.ok(slot===2n || slot===3501n);
        address=slot===2n?roles['1'].address:replaced?p.exchange:roles['3500'].address;
      }
      return '0x'+address.slice(2).padStart(64,'0');
    }};
  }
  await checkAmbient(node(false),plan,at);
  await assert.rejects(checkAmbient(node(true),plan,at),/изменился/);
  await assert.rejects(checkAmbient(node(false),plan,{blockHash:p.sourceBlock}));
});

test('Shipped Ambient CREATE and four exact-output budgets reproduce the funded Ethereum archive',()=>{
  const archived=JSON.parse(gunzipSync(readFileSync(new URL('../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_wallet.json.gz',import.meta.url))));
  const inputs=[archived];
  if(process.env.AMBIENT_REPLAY) inputs.push(JSON.parse(readFileSync(process.env.AMBIENT_REPLAY)));
  const transfer='0x'+sha3.keccak_256('Transfer(address,address,uint256)');
  for(const data of inputs){
    assert.deepEqual(data.manifest,p);
    assert.deepEqual(data.head,{blockHash:p.sourceBlock,requireCanonical:true});
    assert.equal(data.payments.filter(r=>r.mode==='direct').length,8);
    assert.equal(data.payments.filter(r=>r.mode==='v5').length,4);
    const targets=data.payments.filter(r=>r.target);
    assert.equal(targets.length,4);
    for(const r of targets){
      const owner=data.owner.toLowerCase().slice(2).padStart(64,'0');
      let debit=0n;
      for(const log of r.logs){
        if(log.address!==r.input || log.topics[0]!==transfer) continue;
        if(log.topics[1]==='0x'+owner) debit+=BigInt(log.data);
        if(log.topics[2]==='0x'+owner) debit-=BigInt(log.data);
      }
      assert.equal(debit,BigInt(r.spent));
      assert.equal(BigInt(r.budget)-debit,10n);
    }
  }
});

test('A competing profile write preserves the original Ethereum CREATE for reconciliation',async()=>{
  const id='ambient-ethereum@ethereum', key='ambient-wallet:1';
  const rows=new Map(), storage={getItem:k=>rows.get(k)??null,setItem:(k,v)=>rows.set(k,v),removeItem:k=>rows.delete(k)};
  const action=deploymentScope(storage,()=>({id,key,venue:'eth-alias',kind:s.kind,settings:s}));
  const intent={chain:1,nonce:'0x0',from:'owner',creation:p.creationSha256};
  action.beforeOrAfterSend(intent);
  let stored='concurrent address';
  async function compareAndSet(name,scope,value,expected){
    assert.equal(name,'dex.router');
    assert.deepEqual(scope,{kind:'Dex',id});
    if(stored!==expected) throw Error('server conflict');
    stored=value;
  }
  await assert.rejects(action.install('verified address',compareAndSet),/server conflict/);
  assert.equal(stored,'concurrent address');
  assert.deepEqual(action.read(),intent);
});
