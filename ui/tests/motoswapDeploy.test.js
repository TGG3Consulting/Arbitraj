import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan,unsigned} from '../src/lib/walletDeployment.js';
import {checkMotoModule} from '../src/lib/motoswapDeploy.js';

const read=file=>readFileSync(new URL(file,import.meta.url));
const profile=JSON.parse(read('../../config/first_run_dex.json')).find(p=>p.settings['dex.kind']==='motoswap_v2');
const plan=JSON.parse(read('../src/assets/motoswap-ethereum.json'));
const archive=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_wallets.json.gz')));
const active={kind:'motoswap_v2',chainId:profile.chain_id,market:profile.settings['dex.factory'],quoter:profile.settings['dex.quoter'],
  hashes:profile.settings['dex.module_runtime_hashes'],motoRoles:profile.settings['dex.motoswap_roles']};
const word=address=>'0x'+address.slice(2).padStart(64,'0');
test('Motoswap deploy reproduces the paid CREATE and binds both constructor roles',async()=>{
  const prepared=await loadPlan(active,plan);
  assert.equal(prepared.runtimeKeccak,archive.manifest.runtime);
  assert.deepEqual(unsigned(prepared),{chainId:'0x1',data:'0x'+archive.manifest.creation+word(active.market).slice(2)+word(active.quoter).slice(2),value:'0x0'});
  for (const change of [{kind:'mooniswap_v1'},{chainId:8453},{market:active.quoter},{hashes:[]},{motoRoles:active.motoRoles.slice(1)}])
    await assert.rejects(loadPlan({...active,...change},plan));
  await assert.rejects(loadPlan(active,{...plan,data:plan.data+'00'}));
  const tag={blockHash:plan.sourceBlock,requireCanonical:true};
  const provider={request:async({method,params:[call,at]})=>{
    assert.equal(method,'eth_call');assert.deepEqual(at,tag);assert.equal(call.to,archive.module);
    return word(call.data==='0xc45a0155'?active.market:active.quoter);
  }};
  await checkMotoModule(provider,prepared,archive.module,tag);
  await assert.rejects(checkMotoModule({request:async()=>word(active.quoter)},prepared,archive.module,tag));
});

