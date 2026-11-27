import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadPlan, checkMarket, unsigned} from '../src/lib/walletDeployment.js';

const read=file=>JSON.parse(readFileSync(new URL(file,import.meta.url)));
const profiles=read('../../config/first_run_dex.json');
for (const [id,generation] of [['mooniswap','original'],['oneinch-liquidity-protocol','successor']]) {
  test('deployment uses its original factory, WETH and generation: '+id,async()=>{
    const plan=read('../src/assets/mooniswap-'+generation+'-create.json');
    const s=profiles.find(p=>p.id===id).settings;
    const settings={kind:s['dex.kind'],chainId:1,market:s['dex.factory'],factoryHash:s['dex.factory_runtime_hash'],
      wrapped:plan.roles.wrapped.address,hashes:s['dex.module_runtime_hashes']};
    assert.equal(await loadPlan(settings,plan),plan);
    assert.equal(unsigned(plan).data,plan.data);
    await assert.rejects(loadPlan({...settings,wrapped:settings.market},plan));
    await assert.rejects(loadPlan(settings,{...plan,hasSwapFor:!plan.hasSwapFor}));
    await assert.rejects(loadPlan(settings,{...plan,data:'0x1234'+plan.data.slice(6)}));
    await assert.rejects(loadPlan({...settings,chainId:8453},plan));
    const calls=[];
    const provider={request:async ({method,params})=>{
      calls.push([method,params]);
      return method==='eth_chainId'?'0x1':'0x00';
    }};
    const block={blockHash:plan.sourceBlock,requireCanonical:true};
    await assert.rejects(checkMarket(provider,plan,block),/код контракта/);
    assert.deepEqual(calls.filter(([m])=>m==='eth_getCode').map(([,args])=>args),
      [[settings.market,block],[settings.wrapped,block]]);
  });
}
