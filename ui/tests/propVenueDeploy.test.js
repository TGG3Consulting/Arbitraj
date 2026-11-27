import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan,unsigned} from '../src/lib/walletDeployment.js';
import {checkPropVenue,checkPropModule} from '../src/lib/propVenueDeploy.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const read=file=>readFileSync(new URL(file,import.meta.url));
const archive=JSON.parse(read('../src/assets/prop-venue-ethereum.json'));
const profiles=JSON.parse(read('../../config/first_run_dex.json')).filter(p=>p.settings['dex.kind']==='lambda_propamm');
const settings=p=>({chainId:p.chain_id,kind:p.settings['dex.kind'],market:p.settings['dex.factory'],
  quoter:p.settings['dex.quoter'],propRoles:p.settings['dex.propamm_roles'],hashes:p.settings['dex.module_runtime_hashes']});
const word=v=>'0x'+v.slice(2).padStart(64,'0');
const at={blockHash:archive.sourceBlock,requireCanonical:true};

test('each configured venue builds its own CREATE and each wallet runtime matches its funded replay',async()=>{
  for (const profile of profiles){
    const config=settings(profile);
    const p=await loadPlan(config,archive);
    assert.equal(p.market,config.market);
    assert.deepEqual(unsigned(p),{chainId:'0x1',value:'0x0',data:archive.creation+[...p.addresses.map(word),...p.shells].map(x=>x.slice(2)).join('')});
    assert.deepEqual(await loadPlan(config,p),p);
    await assert.rejects(loadPlan({...config,chainId:8453},p));
    await assert.rejects(loadPlan({...config,market:config.quoter},p));
    await assert.rejects(loadPlan({...config,hashes:[]},p));
    await assert.rejects(loadPlan(config,{...p,data:p.data+'00'}));
    const former=config.propRoles.map(r=>r.startsWith('router ')?r.replace(/\S+$/,'0x'+'01'.repeat(32)):r);
    await assert.rejects(loadPlan({...config,propRoles:former},p));
  }
  const accepted=new Set();
  for (const file of ['propamm','kipseli','kaliber','taurus']){
    const rows=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/'+file+'_ethereum_wallets.json.gz'))).rows;
    for (const row of rows.filter(r=>r.status==='paid' && r.form!=='router')){
      assert.equal(row.runtime,archive.markets[row.venue].runtimeKeccak);
      accepted.add(row.venue);
    }
  }
  assert.deepEqual([...accepted].sort(),Object.keys(archive.markets).sort());
});

test('canonical verification checks every implementation slot, runtime and all constructor getters',async()=>{
  const p=await loadPlan(settings(profiles[0]),archive);
  const roles=structuredClone(p.roles);
  const codes={},slots={};
  for (const [n,r] of Object.entries(roles)){
    codes[r.target]='0x60'+n.length.toString(16).padStart(2,'0');r.code=runtimeKeccak(codes[r.target]);
    slots[r.target]=word(r.implementation??'0x0');
    if(r.implementation){codes[r.implementation]='0x6001';r.implCode=runtimeKeccak(codes[r.implementation]);}
  }
  const module='0x'+'23'.repeat(20);
  const words=Object.fromEntries(['0xf887ea40','0x80f55605','0xfbfa77cf','0x7e2d8a92','0x31574546','0xad5ab79b'].map((sig,i)=>
    [sig,word([...p.addresses,...p.shells][i])]));
  const provider={request:async({method,params})=>{
    assert.deepEqual(params.at(-1),at);
    if(method==='eth_getCode')return codes[params[0]];
    if(method==='eth_getStorageAt')return slots[params[0]];
    assert.equal(method,'eth_call');
    if(params[0].to===module)return words[params[0].data];
    assert.deepEqual(params[0],{to:roles.router.target,data:'0x2f61968b'});
    return word(roles.fallback.target);
  }};
  await checkPropVenue(provider,{...p,roles},at);
  await checkPropModule(provider,p,module,at);
  const old=slots[roles.router.target];slots[roles.router.target]=word(module);
  await assert.rejects(checkPropVenue(provider,{...p,roles},at));slots[roles.router.target]=old;
  words['0xad5ab79b']=word(module);
  await assert.rejects(checkPropModule(provider,p,module,at));
  await assert.rejects(checkPropVenue(provider,p,{blockHash:archive.sourceBlock}));
});
