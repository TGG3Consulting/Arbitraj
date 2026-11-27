import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {dexalotVenues} from '../src/lib/dexalotNetwork.js';
import {loadPlan} from '../src/lib/walletDeployment.js';
import {loadDexalot,checkDexalot,addDexalot} from '../src/lib/dexalotDeploy.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const p=JSON.parse(readFileSync(new URL('../src/assets/dexalot-base-create.json',import.meta.url)));
const owner='0x'+'11'.repeat(20), second='0x'+'22'.repeat(20);
const word=a=>a.slice(2).padStart(64,'0');
function config(){
  const rows=Object.values(p.roles).map(r=>[r.address===p.router?'router':'maker',r.address,r.hash,
    ...(r.implementation?[r.implementation.address,r.implementation.hash]:[])].join(' '));
  return {chainId:8453,kind:'dexalot_rfq',wallet:owner,rows};
}
test('Scoped CREATE binds the router and wallet, preserving other wallet modules',async()=>{
  const c=config(),checked=await loadPlan(c,p);
  assert.equal(checked.makers.length,3);
  await assert.rejects(loadPlan({...c,chainId:1},p));
  await assert.rejects(loadPlan({...c,wallet:'0x'+'00'.repeat(20)},p));
  await assert.rejects(loadDexalot(c,{...p,data:p.data+'00'},async()=>p.creationSha256));
  await assert.rejects(loadPlan({...c,rows:c.rows.filter(r=>!r.startsWith('router '))},p));
  const previous=addDexalot(c.rows,p,owner);
  assert.deepEqual(addDexalot(previous,p,second),[...previous,'module '+second+' '+p.runtimeKeccak]);
  assert.strictEqual(addDexalot(previous,p,owner),previous);
});
test('Replacing one maker leaves an independently registered maker available; a replaced router blocks CREATE',async()=>{
  const c=config(),checked=await loadPlan(c,p);
  const roles=Object.fromEntries(Object.entries(p.roles).map(([a,v])=>[a,{...v,hash:runtimeKeccak('0x60'),
    ...(v.implementation?{implementation:{...v.implementation,hash:runtimeKeccak('0x61')}}:{})}]));
  const plan={...checked,roles}, active=plan.makers.find(a=>!roles[a].implementation);
  const tag={blockHash:p.sourceBlock,requireCanonical:true};
  function node(routerChanged){
    return {request:async({method,params})=>{
      assert.deepEqual(params[1]===tag?params[1]:params[2],tag);
      if(method==='eth_getStorageAt')return '0x'+word(routerChanged?owner:roles[p.router].implementation.address);
      if(method==='eth_call')return '0x'+(32).toString(16).padStart(64,'0')+'1'.padStart(64,'0')+word(active);
      return params[0]===roles[p.router].implementation.address?'0x61':'0x60';
    }};
  }
  await checkDexalot(node(false),plan,tag);
  await assert.rejects(checkDexalot(node(true),plan,tag),/реализация/);
  await assert.rejects(checkDexalot(node(false),{...plan,makers:plan.makers.filter(a=>a!==active)},tag),/реестра/);
});

test('manual RFQ key check uses the registered DEX on the network in the recorded price response', () => {
  const sample = JSON.parse(readFileSync(new URL('../../tests/data/first_run_dex_2026-11-26/dexalot_base_indicative.json', import.meta.url)));
  const records = [{id:'base_dex',kind:'Dex'}, {id:'bsc_dex',kind:'Dex'}, {id:'mexc',kind:'Cex'}];
  const values = {'dex.evm_chain_id|Venue|base_dex':String(sample.chain),
    'dex.evm_chain_id|Venue|bsc_dex':56, 'dex.evm_chain_id|Venue|mexc':sample.chain};
  assert.deepEqual(dexalotVenues(records,values),['base_dex']);
  assert.deepEqual(dexalotVenues(records,{}),[]);
});
