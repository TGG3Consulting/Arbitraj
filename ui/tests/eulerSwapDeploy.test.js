import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {loadPlan,unsigned} from '../src/lib/walletDeployment.js';

const read=p=>readFileSync(new URL(p,import.meta.url));
test('DEX settings expose the deployment form for both EulerSwap generations',()=>{
  const page=read('../src/components/DexFamilies.svelte').toString();
  assert.match(page,/\{#if eulerSwap\(store\.values\['dex\.kind\|Dex\|'\+r\.id\]\) \|\|/);
});
const profiles=JSON.parse(read('../../config/first_run_dex.json'));
// Оплаченные до О-40 сборки остаются допустимыми для уже размещённых модулей владельца.
const previousRuntime={
  base:'0x125059924cf19f97a726d29363866c7a0539f55634e75ad3b556488d2f00828b',
  ethereum:'0x5f915756d126543e0d95160e2cd4a0107b8c1f3c625a03ff69a019d52db7fd9c',
};
for (const network of ['base','ethereum']){
  test('EulerSwap validates both factory arguments and the selected profile on '+network, async()=>{
    const plan=JSON.parse(read('../src/assets/eulerswap-'+network+'.json'));
    const paid=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/eulerswap_'+network+'_wallets.json.gz')));
    assert.equal(paid.runtime,plan.runtimeKeccak);
    assert.deepEqual(paid.factories,plan.factories.map(f=>f.address));
    assert.equal(unsigned(plan).value,'0x0');
    for(const version of [1,2]){
      const profile=profiles.find(p=>p.id==='eulerswap-v'+version+'-'+network);
      const k=profile.settings;
      assert.ok(k['dex.module_runtime_hashes'].includes(previousRuntime[network]));
      for (const kind of [k['dex.kind'],k['dex.kind']+'_module']){
        const cfg={kind,chainId:profile.chain_id,market:k['dex.factory'],factoryHash:k['dex.factory_runtime_hash'],hashes:k['dex.module_runtime_hashes']};
        assert.equal(await loadPlan(cfg,plan),plan);
        await assert.rejects(loadPlan(cfg,{...plan,factories:[...plan.factories].reverse()}));
        await assert.rejects(loadPlan({...cfg,hashes:[]},plan));
      }
    }
  });
}

