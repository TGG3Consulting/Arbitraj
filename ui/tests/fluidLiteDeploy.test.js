import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {loadPlan} from '../src/lib/walletDeployment.js';
import {selectFluidCard,checkFluidLite} from '../src/lib/fluidLiteDeploy.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const plan = JSON.parse(readFileSync(new URL('../src/assets/fluid-dex-lite-ethereum.json', import.meta.url)));
const profiles = JSON.parse(readFileSync(new URL('../../config/first_run_dex.json',import.meta.url)));
const preset=profiles.find(x=>x.id==='fluid-dex-lite-ethereum').settings;
const settings={kind:preset['dex.kind'],chainId:1,market:preset['dex.factory'],factoryHash:preset['dex.factory_runtime_hash'],
  quoter:preset['dex.quoter'],quoterHash:preset['dex.quoter_runtime_hash'],wrapped:plan.wrapped,hashes:preset['dex.module_runtime_hashes']};

test('Fluid Lite deployment binds both contracts, Ethereum, WETH and immutable bytecode', async()=>{
  assert.equal(await loadPlan(settings,plan),plan);
  for(const changed of [{kind:'ambient_ethereum'},{chainId:8453},{market:settings.quoter},{quoterHash:settings.factoryHash},
                        {wrapped:settings.market},{hashes:[]}])
    await assert.rejects(loadPlan({...settings,...changed},plan));
  await assert.rejects(loadPlan(settings,{...plan,data:plan.data+'00'}));
});

test('Fluid Lite verifies the original cash and its pricing code at the same canonical head', async()=>{
  const codes={[plan.exchange]:'0x6000',[settings.quoter]:'0x610000'};
  const roles=Object.fromEntries(Object.entries(codes).map(([a,code])=>[a,runtimeKeccak(code)]));
  const at={blockHash:plan.sourceBlock,requireCanonical:true};
  const provider={request:async({method,params})=>{
    assert.equal(method,'eth_getCode'); assert.deepEqual(params[1],at);return codes[params[0]];
  }};
  await checkFluidLite(provider,{...plan,roles},at);
  await assert.rejects(checkFluidLite(provider,{...plan,roles:{...roles,[settings.quoter]:roles[plan.exchange]}},at));
  await assert.rejects(checkFluidLite(provider,plan,{blockHash:at.blockHash}));
});

test('Fluid Lite preserves configured runtime and exact pending or manually supplied creation', async () => {
  const prior = JSON.parse(readFileSync(new URL('../src/assets/fluid-dex-lite-ethereum-previous.json', import.meta.url)));
  const cards = [plan, prior];
  assert.notEqual(plan.runtimeKeccak, prior.runtimeKeccak);
  for (const c of cards) {
    assert.equal(await loadPlan(settings, c), c);
    assert.equal(selectFluidCard(settings, {creation:c.creationSha256}, undefined, cards), c);
    assert.equal(selectFluidCard(settings, null, c.data, cards), c);
    assert.equal(selectFluidCard({...settings, hashes:[c.runtimeKeccak]}, null, undefined, cards), c);
  }
  assert.equal(selectFluidCard(settings, null, undefined, cards), plan);
  assert.throws(() => selectFluidCard(settings, {creation:prior.runtimeSha256}, undefined, cards));
  assert.throws(() => selectFluidCard(settings, null, '0x', cards));
  assert.throws(() => selectFluidCard({...settings, hashes:[plan.runtimeKeccak]}, {creation:prior.creationSha256}, undefined, cards));
});
