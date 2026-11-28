import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {loadPlan,unsigned,hashCode} from '../src/lib/walletDeployment.js';
import {addTextile} from '../src/lib/textileDeploy.js';

const card=JSON.parse(readFileSync(new URL('../src/assets/textile-base-create.json',import.meta.url)));
const rows=Object.entries(card.roles).map(([name,r])=>[8453,name==='fee_controller'?'fee':name,r.address,r.runtime].join(' '));

test('Textile Base uses the recorded CREATE and runtime for each owner',async()=>{
  let registered=rows;
  for(const control of card.controls){
    const module=control===card.controls[0]?'0x0000000000000000000000000000000054455854':'0x0000000000000000000000000000000054455855';
    const settings={kind:card.kind,chainId:8453,wallet:control.owner,rows:registered};
    const p=await loadPlan(settings,card);
    const tx=unsigned(p);
    assert.equal(tx.chainId,'0x2105');
    assert.equal(tx.value,'0x0');
    assert.equal(await hashCode(tx.data),control.creationSha256);
    assert.equal(p.runtimeKeccak,control.runtimeKeccak);
    assert.equal(p.runtimeSha256,control.runtimeSha256);
    registered=addTextile(registered,p,module);
    assert.ok(registered.includes([8453,'wallet',module,control.runtimeKeccak,control.owner].join(' ')));
    assert.strictEqual(addTextile(registered,p,module),registered);
    await assert.rejects(loadPlan({...settings,chainId:56},card));
    await assert.rejects(loadPlan({...settings,rows:rows.slice(1)},card));
    await assert.rejects(loadPlan(settings,{...card,creation:card.creation+'00'}));
  }
  assert.equal(registered.length,rows.length+2);
  assert.notEqual(card.controls[0].runtimeKeccak,card.controls[1].runtimeKeccak);
});
