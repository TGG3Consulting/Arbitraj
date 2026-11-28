import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import sha3 from 'js-sha3';
import {loadPlan,sameRuntime,unsigned,deployed} from '../src/lib/walletDeployment.js';
import {registeredBop,addBop,checkBopModule} from '../src/lib/bopammDeploy.js';
const read=url=>readFileSync(new URL(url,import.meta.url));
const card=JSON.parse(read('../src/assets/bopamm-ethereum-create.json'));
const dir='../../tests/data/first_run_dex_2026-11-26/';
const fact=JSON.parse(gunzipSync(read(dir+'bopamm_ethereum_deploy.json.gz')));
const paid=JSON.parse(gunzipSync(read(dir+'bopamm_ethereum_wallets.json.gz')))[0];
const wallet=paid.payer.toLowerCase(),module=paid.module;
const rows=Object.entries(card.roles).map(([n,r])=>[n,r.address,r.hash].join(' '));
const settings={kind:card.kind,chainId:card.chainId,wallet,rows};
const addrWord=a=>'0x'+a.slice(2).padStart(64,'0');
const sel=name=>'0x'+sha3.keccak_256(name+'()').slice(0,8);
test('Three recorded constructor arguments bind the market, pricing and settlement USDC',async()=>{
  const p=await loadPlan(settings,card);
  assert(unsigned(p).data.endsWith([card.roles.market.address,card.roles.pricing.address,card.usdc].map(a=>addrWord(a).slice(2)).join('')));
  assert(await sameRuntime(fact.moduleCode,p));
  assert.equal(p.runtimeKeccak,paid.cases[0].after.runtime);
  await assert.rejects(loadPlan({...settings,rows:rows.slice(0,2)},card));
  await assert.rejects(loadPlan({...settings,chainId:8453},card));
  await assert.rejects(loadPlan({...settings,rows:[...rows,rows[0]]},card));
  await assert.rejects(loadPlan(settings,{...card,usdc:card.roles.market.address}));
});
function provider(broken) {
  const block=card.sourceBlock,hash='0x'+'af'.repeat(32);
  return {hash,async request({method,params}) {
    switch(method) {
      case 'eth_chainId': return '0x1';
      case 'eth_requestAccounts': return [wallet];
      case 'eth_getBlockByNumber': return {hash:block,number:'0xb',transactions:[hash]};
      case 'eth_getTransactionByHash': return {hash,chainId:'0x1',nonce:'0x0',from:wallet,to:null,value:'0x0',blockHash:block,
        input:card.creation+[card.roles.market.address,card.roles.pricing.address,card.usdc].map(a=>addrWord(a).slice(2)).join('')};
      case 'eth_getTransactionReceipt':return {transactionHash:hash,status:'0x1',contractAddress:module,blockNumber:'0xb',blockHash:block};
      case 'eth_getCode': {
        assert.deepEqual(params[1],{blockHash:block,requireCanonical:true});
        if(params[0]===module) return fact.moduleCode;
        const role=Object.keys(card.roles).find(k=>card.roles[k].address===params[0]);
        assert(role);
        return role===broken?'0x6000':fact.roles[role];
      }
      case 'eth_call':{
        assert.deepEqual(params[1],{blockHash:block,requireCanonical:true});
        const {to,data}=params[0];
        assert([module,card.roles.market.address].includes(to));
        for(const [name,expected] of [['market',card.roles.market.address],['pricing',card.roles.pricing.address],['usdc',card.usdc]]) {
          if(data===sel(name)) return addrWord(broken===to+name?wallet:expected);
        }
        break;
      }
    }
    throw Error('Unexpected RPC '+method);
  }};
}
test('Finalized deployment and reuse verify the module and every configured role; unrelated rows survive',async()=>{
  const p=await loadPlan(settings,card),rpc=provider();
  const installed=addBop(rows,p,module);
  assert.deepEqual(installed.slice(0,-1),rows);
  assert.equal(addBop(installed,p,module),installed);
  assert.equal(await registeredBop(rpc,p,rows),null);
  assert.equal(await registeredBop(rpc,p,installed),module);
  assert.deepEqual(await deployed(rpc,p,rpc.hash,null),{address:module});
  for(const bad of ['registry','market','pricing',module+'market',module+'pricing',module+'usdc',card.roles.market.address+'pricing']) {
    const node=provider(bad);
    await assert.rejects(registeredBop(node,p,installed));
    await assert.rejects(deployed(node,p,node.hash,null));
  }
});

test('The two-leg wallet registers its own role and preserves an existing direct module',async()=>{
  const cash=JSON.parse(read('../src/assets/bopamm-cash-ethereum-create.json'));
  const cashFact=JSON.parse(gunzipSync(read(dir+'bopamm_ethereum_cash_wallet.json.gz')));
  const direct=addBop(rows,card,module);
  const other=cashFact.frames.flatMap(f=>f.cases).map(c=>c.compound.executor).find(x=>x && x!==module);
  const archived=JSON.parse(gunzipSync(read(dir+'bopamm_ethereum_cash_roles.json.gz')))[0];
  const cashRows=[...Object.entries(archived.roles).map(([name,r])=>[name,r.address,r.hash].join(' ')),'fee '+archived.fee];
  const input={...settings,rows:direct,marketBindings:{cashRows}};
  const p=await loadPlan(input,cash);
  for(const bad of ['pool','weth','usdt']) {
    const changed=cashRows.map(row=>row.startsWith(bad+' ')?row.replace(archived.roles[bad].address,wallet):row);
    await assert.rejects(loadPlan({...input,marketBindings:{cashRows:changed}},cash));
  }
  await assert.rejects(loadPlan({...input,marketBindings:{}},cash));
  const role=archived.roles;
  const binds={market:card.roles.market.address,pricing:card.roles.pricing.address,usdc:role.usdc.address,
    WETH:role.weth.address,USDT:role.usdt.address,USDC:role.usdc.address,V3_ROUTER:role.router.address,
    V3_QUOTER:role.quoter.address,POOL_FEE:'0x'+archived.fee.toString(16)};
  const getter=wrong=>({async request({method,params}) {
    assert.equal(method,'eth_call');
    assert.equal(params[0].to,other);
    assert.deepEqual(params[1],archived.selector);
    const [key,value]=Object.entries(binds).find(([name])=>sel(name)===params[0].data);
    return addrWord(wrong===key?wallet:value);
  }});
  await checkBopModule(getter(),p,other,archived.selector);
  await assert.rejects(checkBopModule(getter('WETH'),p,other,archived.selector));
  await assert.rejects(checkBopModule(getter('POOL_FEE'),p,other,archived.selector));
  assert(await sameRuntime(cashFact.runtime,p));
  const both=addBop(direct,p,other);
  assert.deepEqual(both.slice(0,-1),direct);
  assert.equal(both.at(-1),'cash_module '+other+' '+cash.runtimeKeccak);
  assert.deepEqual(addBop(both,p,other),both);
  await assert.rejects(loadPlan({...settings,chainId:8453},cash));
});
