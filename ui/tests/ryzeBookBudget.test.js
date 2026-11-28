// Размеры и общая частота Router из поставляемого профиля Ryze. node:test, записанные рынки и ответы Base.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {baseNetwork} from '../src/lib/baseNetwork.js';
import {ryzeBookBudget} from '../src/lib/ryzeBookBudget.js';

const read=path=>readFileSync(new URL(path,import.meta.url),'utf8');
const records=read('../../config/first_run.toml').split(/^\[\[section\.rec\]\]\r?$/m);
const dir='../../tests/data/first_run_dex_2026-11-26/';
const service=JSON.parse(read(dir+'ryze_service_responses.json'));
const control=JSON.parse(read(dir+'ryze_buy_gross_control.json'));
const chains=JSON.parse(read('../../tests/data/base_ethereum_networks.json'));
const profiles=Object.entries(chains).map(([chain,r])=>({venue:chain==='base'?'base_dex':'eth_dex',chain,evm_chain_id:r.chain}));

/** Читает строковое значение записи первого запуска в указанной области. */
function recommendation(name,area) {
  const record=records.find(row=>row.includes('name = "'+name+'"') && row.includes('area = "'+area+'"'));
  return JSON.parse(record.match(/^value = (".*")\r?$/m)[1]);
}
const rows=recommendation('ryze.quote_sizes','Venue base_dex').split(',');
const interval=Number(recommendation('ryze.books_interval_ms','Venue base_dex'));
const budget=Number(recommendation('ryze.router_requests_per_minute','вся система'));
const global='ryze.router_requests_per_minute|Global|';
const budgetFor=(venue,rows,interval)=>({
  ['ryze.quote_sizes|Venue|'+venue]:rows,
  ['ryze.books_interval_ms|Venue|'+venue]:interval,
  [global]:budget,
});

test('Ryze Base takes each quoted size once, including several sizes for the same token',()=>{
  assert.deepEqual(rows.map(row=>row.split('=')[0]),service.settings_pools.map(([,asset])=>asset));
  assert.equal(control.calls.length,4);
  assert.deepEqual(control.calls.map(c=>Object.hasOwn(c.request,'amountIn')),[false,true,true,true]);
  const cfg=budgetFor('base_dex',rows,interval);
  assert.deepEqual(ryzeBookBudget(cfg,'base_dex'),{sizes:2,buy:8,sell:2,total:10,perMinute:60,cap:100,mayExhaust:false});
  const secondSale=service.router_quotes.find(q=>!q.buy).calls[1].request;
  const otherSize=secondSale.tokenIn+'=0.'+secondSale.amountIn.padStart(18,'0');
  assert.equal(secondSale.tokenIn,service.settings_pools[0][1]);
  assert.ok(!rows.includes(otherSize));
  const multiple=budgetFor('base_dex',[...rows,otherSize],interval);
  assert.equal(ryzeBookBudget(multiple,'base_dex').total,15);
  assert.equal(ryzeBookBudget({...multiple,[global]:75},'base_dex').mayExhaust,true);
  assert.equal(ryzeBookBudget({...multiple,'ryze.books_interval_ms|Venue|base_dex':120000,[global]:14},'base_dex').mayExhaust,true);
});

test('A renamed Base venue uses its own sizes, while the Router budget has only global scope',()=>{
  const alias='spot_sidechain';
  const cfg={...budgetFor('base_dex',rows,interval),...budgetFor(alias,rows.slice(1),interval),
    ['dex.chain|Venue|'+alias]:'base',['dex.evm_chain_id|Venue|'+alias]:chains.base.chain,
    'dex.chain|Venue|eth_dex':'ethereum','dex.evm_chain_id|Venue|eth_dex':chains.ethereum.chain,
    'ryze.router_requests_per_minute|Venue|spot_sidechain':1,
    'ryze.relayer_requests_per_minute|Global|':5};
  assert.equal(baseNetwork(alias,profiles,cfg),chains.base.chain);
  assert.equal(baseNetwork('eth_dex',profiles,cfg,true),null);
  assert.equal(baseNetwork('bsc_dex',profiles,cfg,true),null);
  assert.equal(baseNetwork(alias,profiles,{...cfg,['dex.chain|Venue|'+alias]:'ethereum'},true),null);
  assert.equal(baseNetwork('base_dex',profiles,{},true),chains.base.chain);
  assert.equal(baseNetwork('base_dex',profiles,{}),null);
  assert.deepEqual(ryzeBookBudget(cfg,alias),{sizes:1,buy:4,sell:1,total:5,perMinute:30,cap:100,mayExhaust:false});
  for (const invalid of [undefined,0,-1,2.5,'100']) {
    const noBudget=ryzeBookBudget({...cfg,[global]:invalid},alias);
    assert.equal(noBudget.cap,null);
    assert.equal(noBudget.perMinute,null);
  }
  assert.equal(ryzeBookBudget({...cfg,['ryze.books_interval_ms|Venue|'+alias]:0},alias).perMinute,null);
});
