import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {usesOriginalAktionariat} from '../src/lib/aktionariatDeploy.js';
import {checkMarket,loadPlan} from '../src/lib/walletDeployment.js';

const read=file=>JSON.parse(readFileSync(new URL(file,import.meta.url)));
const archive=read('../src/assets/aktionariat-ethereum.json');
const profile=read('../../config/first_run_dex.json').find(x=>x.id==='aktionariat-ethereum').settings;
const settings={kind:profile['dex.kind'],chainId:1,market:profile['dex.factory'],factoryHash:profile['dex.factory_runtime_hash'],
  quoter:profile['dex.quoter'],quoterHash:profile['dex.quoter_runtime_hash'],hashes:profile['dex.module_runtime_hashes'],
  brokerMarkets:profile['dex.broker_markets']};

test('Ethereum CREATE binds the complete list of issuer markets, their cash and share contracts',async()=>{
  assert.equal(await loadPlan(settings,archive),archive);
  await assert.rejects(loadPlan({...settings,chainId:8453},archive));
  await assert.rejects(loadPlan({...settings,brokerMarkets:[]},archive));
  const changed=structuredClone(archive);
  changed.issuerMarkets.reverse();changed.issuerRoles.reverse();
  await assert.rejects(loadPlan(settings,changed));
  const issuer=archive.issuerRoles[0];
  await assert.rejects(loadPlan({...settings,brokerMarkets:settings.brokerMarkets.map(row=>row.replace(issuer.owner,issuer.cash))},archive));
});

test('a different network rejects the deployment before reading any role',async()=>{
  const seen=[];
  const provider={request:({method})=>{seen.push(method);return '0x2105';}};
  await assert.rejects(checkMarket(provider,archive,{requireCanonical:true,blockHash:archive.sourceBlock}));
  assert.deepEqual(seen,['eth_chainId']);
});

test('saved module fingerprint selects the original CREATE; new profiles select the accepted update',async()=>{
  const original=read('../src/assets/aktionariat-ethereum-original.json');
  const saved={...settings,hashes:[original.runtimeKeccak]};
  assert.equal(usesOriginalAktionariat(saved.hashes),true);
  assert.equal(usesOriginalAktionariat(settings.hashes),false);
  assert.equal(await loadPlan(saved,original),original);
  await assert.rejects(loadPlan(saved,archive));
  await assert.rejects(loadPlan(settings,original));
});
