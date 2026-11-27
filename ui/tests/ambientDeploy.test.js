import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import sha3 from 'js-sha3';
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
                       {ambientRoles:[...s.ambientRoles,s.ambientRoles[0]]}])
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
