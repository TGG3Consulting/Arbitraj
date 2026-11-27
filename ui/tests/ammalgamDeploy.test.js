import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadPlan,unsigned} from '../src/lib/walletDeployment.js';
const read=path=>JSON.parse(readFileSync(new URL(path,import.meta.url)));
const p=read('../src/assets/ammalgam-ethereum-create.json');
const profile=read('../../config/first_run_dex.json').find(r=>r.id==='ammalgam' && r.chain_id===1).settings;
const s={kind:profile['dex.kind'],chainId:1,market:profile['dex.factory'],factoryHash:profile['dex.factory_runtime_hash'],
  quoter:profile['dex.quoter'],quoterHash:profile['dex.quoter_runtime_hash'],cloneHash:profile['dex.pair_runtime_hash'],
  hashes:profile['dex.module_runtime_hashes'],implementations:profile['dex.proxy_implementations'],implementationHashes:profile['dex.proxy_runtime_hashes']};
test('only the funded Ammalgam Ethereum deployment can prefill this market',async()=>{
  assert.deepEqual(unsigned(await loadPlan(s,p)),{chainId:'0x1',data:p.data,value:'0x0'});
  for(const changed of [{chainId:8453},{kind:'mooniswap_v1'},{quoter:s.market},{implementations:[...s.implementations].reverse()}]){
    await assert.rejects(loadPlan({...s,...changed},p));
  }
  await assert.rejects(loadPlan(s,{...p,executorHash:p.roles.helper.keccak256}));
});
