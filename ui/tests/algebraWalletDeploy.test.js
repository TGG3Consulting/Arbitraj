import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadPlan,checkMarket} from '../src/lib/walletDeployment.js';
import {deploymentScope} from '../src/lib/deploymentScope.js';
const read=path=>JSON.parse(readFileSync(new URL(path,import.meta.url)));
const plan=read('../src/assets/theorem-ethereum.json');
const settings=read('../../config/first_run_dex.json').find(p=>p.id==='theorem-ethereum').settings;
const view={kind:settings['dex.kind'],chainId:1,market:settings['dex.factory'],factoryHash:settings['dex.factory_runtime_hash'],
  officialRouter:settings['dex.router'],routerHash:settings['dex.router_runtime_hash'],deployer:settings['dex.algebra_pool_deployer'],
  hashes:settings['dex.module_runtime_hashes']};

test('Theorem binds both constructor roles and installs the optional module separately',async()=>{
  assert.equal(await loadPlan(view,plan),plan);
  for (const change of [{chainId:8453},{market:view.officialRouter},{officialRouter:view.market},
      {routerHash:view.factoryHash},{hashes:[]},{deployer:view.market}]) {
    await assert.rejects(loadPlan({...view,...change},plan));
  }
  await assert.rejects(loadPlan(view,{...plan,data:plan.data.slice(0,-2)+'00'}));
  const id='theorem-ethereum@ethereum', kind=view.kind, writes=[];
  deploymentScope({},()=>({id,kind,key:'theorem',venue:'ethereum',settings:view}))
    .install('accepted',(...args)=>writes.push(args));
  assert.deepEqual(writes,[['dex.v3_wallet_module',{kind:'Dex',id},'accepted']]);
});

test('Theorem checks all three code hashes and both deployer getters on one canonical block',async()=>{
  const {runtimeKeccak}=await import('../src/lib/uniswapV1Deploy.js');
  const code={[plan.factory]:'0x11',[plan.router]:'0x22',[plan.deployer]:'0x33'};
  const p={...plan,factoryHash:runtimeKeccak('0x11'),routerHash:runtimeKeccak('0x22'),deployerHash:runtimeKeccak('0x33')};
  const at={blockHash:plan.sourceBlock,requireCanonical:true};
  let wrong=false, reads=0;
  const provider={request:async({method,params})=>{
    if(method==='eth_chainId') return '0x1';
    assert.deepEqual(params[1],at);
    if(method==='eth_getCode') return code[params[0]];
    assert.equal(method,'eth_call');reads++;
    const target=params[0].data==='0xc45a0155'?p.factory:
      wrong && params[0].to===p.router?p.factory:p.deployer;
    return '0x'+target.slice(2).padStart(64,'0');
  }};
  await checkMarket(provider,p,at);
  assert.equal(reads,3);
  wrong=true;
  await assert.rejects(checkMarket(provider,p,at));
});
