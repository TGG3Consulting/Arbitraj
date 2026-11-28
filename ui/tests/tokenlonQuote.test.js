import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
import test from 'node:test';
import {previewRequest} from '../src/lib/tokenlonQuote.js';

const {assets} = JSON.parse(readFileSync(new URL('../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_roles.json', import.meta.url)));
const tether = assets.find(row => row.split(' ')[1] === 'tether-net').split(' ')[2];
const {controls} = JSON.parse(readFileSync(new URL('../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum.json', import.meta.url)));
test('wallet preview keeps the paid direction, base amount and net minimum from Ethereum PMM settlements', () => {
  const directions = new Set();
  for (const row of controls.filter(row => row.src === tether || row.dst === tether)) {
    const buy = row.src === tether;
    const token = buy ? row.dst : row.src;
    const size = buy ? row.maker : row.taker;
    const form = {cash:tether,token,buy,size,firm:true,minimum:row.net,maximum:row.taker,wallet:'w12',trader:row.user};
    const body = previewRequest(form);
    assert.equal(body.input,row.src);
    assert.equal(body.output,row.dst);
    assert.equal(body[buy ? 'buy_amount_raw' : 'amount_raw'],size);
    assert.equal(body.firm.minimum_raw,row.net);
    assert.equal(body.firm.maximum_raw,buy ? row.taker : undefined);
    assert.equal(body.firm.allow_trade,false);
    assert.equal(previewRequest({...form,firm:false}).firm,undefined);
    directions.add(buy);
  }
  assert.deepEqual([...directions].sort(),[false,true]);
});
test('wrong assets and unrepresentable amounts cannot become a request', () => {
  const row = controls[0];
  const form = {cash:tether,token:row.src,trader:row.user,wallet:'w12',size:'1'};
  for (const change of [{token:tether.toUpperCase().replace('0X','0x')},{trader:''},{wallet:''},{size:'0'},{size:'1.5'},{size:(2n**256n).toString()}]) {
    assert.equal(previewRequest({...form,...change}),null);
  }
});
