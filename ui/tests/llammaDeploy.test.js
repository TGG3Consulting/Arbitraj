import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {loadPlan, unsigned} from '../src/lib/walletDeployment.js';

const json = path => JSON.parse(readFileSync(new URL(path,import.meta.url)));
const profiles = json('../../config/first_run_dex.json');
for (const alias of ['oneway','mint','v2']) {
  const p = json('../src/assets/llamma-'+alias+'-create.json');
  test('Curve LLAMMA '+alias+' matches its Ethereum registry and funded constructor',async()=>{
    const row = profiles.find(r=>r.chain_id===1 && r.settings['dex.kind']===p.kind).settings;
    const s = {kind:p.kind,chainId:1,market:row['dex.factory'],factoryHash:row['dex.factory_runtime_hash'],
      hashes:row['dex.module_runtime_hashes']};
    assert.deepEqual(unsigned(await loadPlan(s,p)),{chainId:'0x1',value:'0x0',data:p.data});
    for (const change of [{chainId:8453},{factoryHash:p.runtimeKeccak},{hashes:[]}]) {
      await assert.rejects(loadPlan({...s,...change},p));
    }
    await assert.rejects(loadPlan(s,{...p,tupleRegistry:!p.tupleRegistry}));
    await assert.rejects(loadPlan(s,{...p,data:p.data+'00'}));
  });
}
