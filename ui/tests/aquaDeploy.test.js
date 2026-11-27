import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {loadAqua,unsignedCache} from '../src/lib/aquaDeploy.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const sha=async hex=>createHash('sha256').update(Buffer.from(hex.slice(2),'hex')).digest('hex');
for (const network of ['base','ethereum']) {
  const p=JSON.parse(await readFile(new URL('../src/assets/aqua-'+network+'-create.json',import.meta.url)));
  const s={kind:p.kind,chainId:p.chainId,hashes:[p.runtimeKeccak],market:p.roles.registry.address,
    factoryHash:p.roles.registry.keccak256,quoter:p.roles.engine.address,quoterHash:p.roles.engine.keccak256};
  test('Aqua deployment and original registry bindings on '+network,async()=>{
    assert.equal(await loadAqua(s,p,sha),p);
    await assert.rejects(loadAqua({...s,chainId:56},p,sha));
    await assert.rejects(loadAqua({...s,quoterHash:p.roles.registry.keccak256},p,sha));
    await assert.rejects(loadAqua(s,{...p,data:p.data.slice(0,-2)+'00'},sha));
    for(const entry of p.cache) {
      assert.equal(runtimeKeccak('0x'+entry.data.slice(10)),entry.order);
      assert.equal(entry.tokens.length,2);
    }
    const bad={...p.cache[0],order:'0x'+'00'.repeat(32)};
    await assert.rejects(unsignedCache({},p,'0x'+'11'.repeat(20),bad),/программа/);
  });
}
