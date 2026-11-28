// Артефакт GitHub 45867180736. Имитация квитанции CREATE проверяет восстановление интерфейса.
import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import sha3 from 'js-sha3';
import {deployed,loadPlan,sameRuntime,unsigned} from '../src/lib/walletDeployment.js';
import {addFermi,fermiNetwork,registeredFermi} from '../src/lib/fermiDeploy.js';

const bytes=p=>readFileSync(new URL(p,import.meta.url));
const card=JSON.parse(bytes('../src/assets/fermi-ethereum-create.json'));
const fixtures='../../tests/data/first_run_dex_2026-11-26/';
const facts=JSON.parse(gunzipSync(bytes(fixtures+'fermi_ethereum_deploy.json.gz')));
const receipts=JSON.parse(bytes(fixtures+'fermi_slot_receipts.json'));
const wallet=receipts.cases[0].payer, target=receipts.executor;
const settings={kind:'fermi',chainId:1,wallet,marketBindings:card};
const word=a=>'0x'+a.toLowerCase().slice(2).padStart(64,'0');

test('Ethereum settings and both hashes bind the exact contract paid in two builder frames',async()=>{
  const meta=[{venue:'eth_dex',chain:'ethereum',evm_chain_id:1}];
  const values={'dex.chain|Venue|eth_dex':'ethereum','dex.evm_chain_id|Venue|eth_dex':1};
  assert.equal(fermiNetwork('eth_dex',meta,values),1);
  assert.equal(fermiNetwork('base_dex',meta,values),null);
  const plan=await loadPlan(settings,card);
  assert.equal(unsigned(plan).data,card.data);
  assert(await sameRuntime(facts.moduleCode,plan));
  assert.equal(plan.runtimeKeccak,receipts.runtime_keccak);
  assert.deepEqual([...new Set(receipts.cases.map(r=>r.builder))].sort(),['BuilderNet','Titan']);
  assert.equal(receipts.cases.filter(r=>r.receipt.status==='0x1').length,8);
  await assert.rejects(loadPlan(settings,{...card,data:card.data+'00'}));
  await assert.rejects(loadPlan({...settings,chainId:8453},card));
  await assert.rejects(loadPlan({...settings,marketBindings:{...card,engine:card.router}},card));
});

function fixture(changed={}) {
  const block=card.sourceBlock, tx='0x'+'fb'.repeat(32);
  const selector=name=>'0x'+sha3.keccak_256(name).slice(0,8);
  return {tx,async request({method,params}) {
    switch(method) {
      case 'eth_chainId': return '0x1';
      case 'eth_requestAccounts': return [wallet];
      case 'eth_getBlockByNumber': return {hash:block,number:'0xb',transactions:[tx]};
      case 'eth_getTransactionByHash': return {hash:tx,chainId:'0x1',nonce:'0x0',from:wallet,to:null,value:'0x0',blockHash:block,input:card.data};
      case 'eth_getTransactionReceipt': return {transactionHash:tx,status:'0x1',contractAddress:target,blockNumber:'0xb',blockHash:block};
      case 'eth_getCode':
        assert.deepEqual(params[1],{blockHash:block,requireCanonical:true});
        assert([card.router,target].includes(params[0]));
        return params[0]===card.router ? (changed.routerCode??facts.routerCode) : facts.moduleCode;
      case 'eth_call': {
        assert.deepEqual(params[1],{blockHash:block,requireCanonical:true});
        const {to,data}=params[0];
        if(to===card.router && data===selector('fermi()')) return word(changed.marketEngine??card.engine);
        if(to===target && data===selector('ROUTER()')) return word(card.router);
        if(to===target && data===selector('ENGINE()')) return word(changed.moduleEngine??card.engine);
        break;
      }
    }
    throw Error('Unexpected fixture RPC '+method);
  }};
}

test('Restoring CREATE and checking an installed Fermi module use the same canonical roles',async()=>{
  const plan=await loadPlan(settings,card),rpc=fixture();
  assert.equal(await registeredFermi(rpc,plan,null),null);
  assert.equal(await registeredFermi(rpc,plan,target),target);
  const pending={chain:1,from:wallet,nonce:'0x0',creation:card.creationSha256};
  assert.deepEqual(await deployed(rpc,plan,rpc.tx,pending),{address:target});
  assert.equal(addFermi(null,plan,target),target);
  assert.throws(()=>addFermi(card.engine,plan,target));
  for (const changed of [{routerCode:facts.moduleCode},{marketEngine:target},{moduleEngine:card.router}]) {
    const other=fixture(changed);
    await assert.rejects(registeredFermi(other,plan,target));
    await assert.rejects(deployed(other,plan,other.tx,pending));
  }
});
