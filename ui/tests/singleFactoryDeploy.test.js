import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadPlan, checkMarket} from '../src/lib/walletDeployment.js';
import {deploymentScope} from '../src/lib/deploymentScope.js';

const read=file=>JSON.parse(readFileSync(new URL(file,import.meta.url)));
const rows=read('../../config/first_run_dex.json');
for(const [id,file,chain] of [
  ['shibaswap-v2-ethereum','shibaswap-v2-ethereum',1],
  ['empire-module-ethereum','empire-ethereum',1],['wagmi-base','wagmi-community-base',8453],
  ['thick-base','thick-community-base',8453],['goblins-base','goblins-community-base',8453],
]){
  const plan=read('../src/assets/'+file+'.json');
  const row=rows.find(r=>r.id===id).settings;
  const settings={kind:row['dex.kind'],chainId:chain,market:row['dex.factory'],
    factoryHash:row['dex.factory_runtime_hash'],hashes:row['dex.module_runtime_hashes']};
  test(id+' checks the factory, constructor and accepted runtime',async()=>{
    assert.equal(await loadPlan(settings,plan),plan);
    for(const change of [{chainId:0},{hashes:[]},{market:plan.sourceBlock},{factoryHash:plan.runtimeKeccak}]){
      await assert.rejects(loadPlan({...settings,...change},plan));
    }
    await assert.rejects(loadPlan(settings,{...plan,data:plan.data.slice(0,-2)+'ff'}));
    const provider={request:async({method})=>method==='eth_chainId'?chain:'0x01'};
    await assert.rejects(checkMarket(provider,plan,{blockHash:plan.sourceBlock,requireCanonical:true}));
  });
  test(id+' installs the finalized module at its own settings key',()=>{
    const scope=deploymentScope({},()=>({id,kind:settings.kind,settings,key:file,venue:chain}));
    const writes=[];
    scope.install('module',(...args)=>writes.push(args));
    assert.deepEqual(writes,[[chain===1?'dex.router':'dex.v3_wallet_module',{kind:'Dex',id},'module',null]]);
  });
}
