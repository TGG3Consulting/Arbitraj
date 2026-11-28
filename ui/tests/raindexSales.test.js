// Проверка статусов карточек по сырым Transfer кошелька в архиве Base. Запускает node:test.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import sha3 from 'js-sha3';

const read=p=>readFileSync(new URL(p,import.meta.url));
const inventory=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/raindex_base_deployment_balances.json.gz')));
const card=JSON.parse(read('../src/assets/raindex-base-create.json'));
const transfer='0x'+sha3.keccak256('Transfer(address,address,uint256)');

/** Возвращает чистое изменение баланса токена выбранного участника по ERC20 Transfer. */
function cash(row,token,owner){
  const topic='0x'+owner.slice(2).padStart(64,'0');
  let amount=0n;
  for(const log of row.logs){
    if(log.address!==token || log.topics[0]!==transfer) continue;
    if(log.topics[1]===topic) amount-=BigInt(log.data);
    if(log.topics[2]===topic) amount+=BigInt(log.data);
  }
  return amount;
}

test('Only the measured Raindex sale direction receives a payout or revert status',()=>{
  const run=inventory.sample_replay;
  const books=new Map(card.books.filter(b=>b.sale).map(b=>[b.book,b]));
  assert.equal(books.size,5);
  assert.equal(card.books.filter(b=>b.settlementVerified===null).length,31);
  const expected={
    '0x2aee87d75cd000583daec7a28db103b1c0c18b76':['WLTH','516440961769448410','5164409617694484095'],
    '0x498ff70c5f67e63e53b74551de31387cf2813986':['WETH','299000000000','2990000000000']
  };
  for(const [mode,result] of [['direct',run.direct],['protected',run.protected]]){
    assert.deepEqual(new Set(result.cases.map(r=>r.book)),new Set(books.keys()));
    for(const [addr,b] of books){
      const rows=result.cases.filter(r=>r.book===addr);
      assert.deepEqual(new Set(rows.map(r=>r.size)),new Set(['1000','10000']));
      assert.equal(b.projectFundingVerified,null);
      for(const r of rows){
        assert.equal(r.receive,inventory.tokens.USDC);
        assert.notEqual(r.pay,inventory.tokens.USDC);
        assert.equal(b.sale.input,r.pay);
        assert.equal(b.sale.output,r.receive);
        if(expected[addr]){
          const [symbol,small,large]=expected[addr];
          assert.equal(r.stage,'paid');
          assert.equal(b.sale.asset,symbol);
          assert.equal(b.sale.paid,true);
          assert.equal(b.settlementVerified,true);
          assert.equal(r.paid,r.size==='1000'?small:large);
          assert.equal(cash(r,r.pay,result.payer),-BigInt(r.paid));
          assert.equal(cash(r,inventory.tokens.USDC,result.payer),BigInt(r.size));
          assert.equal(r.target,mode==='direct'?addr:result.modules[addr].address);
        } else {
          assert.equal(r.stage,'revert');
          assert.equal(b.settlementVerified,false);
          assert.equal(b.sale.paid,false);
          assert.equal(b.sale.reason,b.version===4?'min ratio':'Panic(0x32)');
        }
      }
    }
  }
});
