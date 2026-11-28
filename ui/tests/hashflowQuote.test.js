// Ethereum: сделка 0xafe6…d147 и независимо записанные чтения Router/Pool того же блока.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {sameHashflowWallet} from '../src/lib/hashflowQuote.js';

const read=name=>JSON.parse(readFileSync(new URL('../../tests/data/hashflow_direct_2026-11-26/'+name,import.meta.url)));
test('Hashflow retains a measured result only for the selected account and venue',()=>{
  const record=read('ethereum_router.json');
  const trade=read('swaps.json').find(x=>x.transaction_hash===record.tx);
  assert.equal(trade.block_hash,record.blockHash);
  const form={venue:'alternate',trader:trade.from.toLowerCase(),chain:trade.chain_id};
  const result={trader:trade.from,chain_id:trade.chain_id,target:trade.to,pool:trade.trade_logs[0].address};
  assert.equal(sameHashflowWallet(form,'alternate',trade.from,1,result),true);
  assert.equal(sameHashflowWallet(form,'alternate',trade.from,56,result),false);
  assert.equal(sameHashflowWallet(form,'other',trade.from,1,result),false);
  assert.equal(sameHashflowWallet(form,'alternate',record.pool,1,result),false);
  assert.equal(sameHashflowWallet(form,'alternate',trade.from,1,{...result,trader:record.pool}),false);
});
