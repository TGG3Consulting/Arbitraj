import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {loadPlan,unsigned} from '../src/lib/walletDeployment.js';
import {addModule,checkAirSwapOwner} from '../src/lib/airswapDeploy.js';

const wallet='0x'+'12'.repeat(20);
const first='0x'+'23'.repeat(20);
const second='0x'+'34'.repeat(20);
for (const form of ['base-v5','ethereum-v4','ethereum-v5']) {
  const p=JSON.parse(readFileSync(new URL('../src/assets/airswap-'+form+'-create.json',import.meta.url)));
  test('AirSwap '+form+' binds the source code, chain and owner and preserves other registered wallets',async()=>{
    const base=[String(p.chainId),p.kind.slice(-2),p.exchange,p.exchangeHash].join(' ');
    const s={rows:[base],chainId:p.chainId,wallet,kind:p.kind};
    const plan=await loadPlan(s,p);
    assert.deepEqual(unsigned(plan),{chainId:'0x'+p.chainId.toString(16),data:p.data,value:'0x0'});
    assert.equal(plan.wallet,wallet);
    await assert.rejects(loadPlan({...s,wallet:first.slice(1)},p));
    await assert.rejects(loadPlan({...s,chainId:56},p));
    await assert.rejects(loadPlan({...s,rows:[base.replace(p.exchangeHash,p.runtimeKeccak)]},p));
    await assert.rejects(loadPlan(s,{...p,data:p.data+'00'}));
    const added=addModule(s.rows,p,first);
    const both=addModule(added,p,second);
    assert.equal(both.length,3);
    assert.deepEqual(both.slice(0,2),added);
    assert.strictEqual(addModule(both,p,first),both);
    const at={blockHash:p.sourceBlock,requireCanonical:true};
    const request=async q=>{
      assert.deepEqual(q.params,[{to:first,data:'0x8da5cb5b'},at]);
      return '0x'+wallet.slice(2).padStart(64,'0');
    };
    await checkAirSwapOwner({request},plan,first,at);
    await assert.rejects(checkAirSwapOwner({request},{...plan,wallet:second},first,at));
  });
}
