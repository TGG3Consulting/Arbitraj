import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {loadPlan} from '../src/lib/walletDeployment.js';

const read = (file) => JSON.parse(readFileSync(new URL(file, import.meta.url)));
const manifest=read('../src/assets/angstrom-ethereum.json');
const row=read('../../config/first_run_dex.json').find(x=>x.id==='angstrom-ethereum').settings;
const settings={kind:row['dex.kind'],chainId:1,poolManager:row['dex.pool_manager'],
  launchRoles:row['dex.launch_roles'],hashes:row['dex.module_runtime_hashes']};

test('Angstrom CREATE binds the constructor targets, configuration and Ethereum',async()=>{
  assert.equal(await loadPlan(settings,manifest),manifest);
  await assert.rejects(loadPlan({...settings,chainId:8453},manifest));
  await assert.rejects(loadPlan({...settings,launchRoles:[]},manifest));
  await assert.rejects(loadPlan({...settings,poolManager:manifest.roles.hook.address},manifest));
  await assert.rejects(loadPlan(settings,{...manifest,data:manifest.data.slice(0,-2)+'ff'}));
});
