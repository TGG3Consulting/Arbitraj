// Карточка Hanji: архивный CREATE и WETH, повторное подключение к общему адресу и журнал браузера.
// Запускается node:test в GitHub Actions. Восстановление сессии моделируется отдельно от архивных денежных ног.
import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {deployed,loadPlan,unsigned,sameRuntime} from '../src/lib/walletDeployment.js';
import {deploymentScope,sourceDeploymentKey} from '../src/lib/deploymentScope.js';
import {addHanji,hanjiNetwork,registeredHanji} from '../src/lib/hanjiDeploy.js';

const read = path => JSON.parse(readFileSync(new URL(path,import.meta.url)));
const card = read('../src/assets/hanji-base-create.json');
const dir = '../../tests/data/first_run_dex_2026-11-26/';
const fills = read(dir+'hanji_executor_receipts.json');
const wrapped = read(dir+'hanji_wrapped_base.json');
const {payer:wallet,executor:target,market} = fills.evidence[0].order;
const args = {kind:card.kind,wallet,wrapped:card.wrapped,chainId:card.chainId};
const venue = 'base_dex';
const word = address => '0x'+address.slice(2).padStart(64,'0');
const runtime = '0x'+card.data.slice(-(card.runtimeBytes*2+64),-64);

test('Hanji uses the Base profile, the archived creation argument and both runtime hashes',async()=>{
  const profile={venue,chain:'base',evm_chain_id:card.chainId};
  const values={'dex.chain|Venue|base_dex':'base','dex.evm_chain_id|Venue|base_dex':card.chainId};
  assert.equal(hanjiNetwork(venue,[profile],values),card.chainId);
  assert.equal(hanjiNetwork(venue,[profile],{}),null);
  assert.equal(hanjiNetwork(venue,[profile],{...values,'dex.chain|Venue|base_dex':'bsc'}),null);
  assert.equal(hanjiNetwork('bsc_dex',[profile],values),null);
  const plan=await loadPlan(args,card);
  assert.equal(plan.runtimeKeccak,fills.runtime_keccak);
  assert.equal(plan.sourceBlock,fills.block_hash);
  assert.equal(plan.wrapped,wrapped.params[0]);
  assert.equal(plan.sourceBlock,wrapped.params[1].blockHash);
  assert.equal(await sameRuntime(runtime,plan),true);
  assert.deepEqual(unsigned(plan),{chainId:'0x2105',value:'0x0',data:card.data});
  assert.ok(card.data.endsWith(word(card.wrapped).slice(2)));
  await assert.rejects(loadPlan({...args,chainId:56},card));
  await assert.rejects(loadPlan({...args,wrapped:market},card));
  await assert.rejects(loadPlan(args,{...card,data:card.data+'00'}));
});

// Ответ о WETH — записанный эталон узла. CREATE-квитанция и nonce ниже моделируют перезапуск клиента:
// реального размещения владельцем в архиве нет.
function replay(getter) {
  const hash=card.runtimeKeccak;
  const block={hash:card.sourceBlock,number:'0x10',transactions:[hash]};
  const chain='0x2105', nonce='0x0';
  const receipt={transactionHash:hash,blockHash:block.hash,blockNumber:block.number,status:'0x1',contractAddress:target};
  const tx={hash,blockHash:block.hash,from:wallet,nonce,to:null,value:'0x0',chainId:chain,input:card.data};
  const request=async({method,params})=>{
    switch(method){
      case 'eth_chainId': return chain;
      case 'eth_requestAccounts': return [wallet];
      case 'eth_getBlockByNumber': return block;
      case 'eth_getTransactionReceipt': assert.deepEqual(params,[hash]); return receipt;
      case 'eth_getTransactionByHash': assert.deepEqual(params,[hash]); return tx;
      case 'eth_getCode':
        assert.deepEqual(params[1],wrapped.params[1]);
        if (params[0]===target) return runtime;
        assert.equal(params[0],card.wrapped);
        return wrapped.response.result;
      case 'eth_call':
        assert.equal(params[0].to,target);
        assert.equal(params[0].data,'0x50e70d48'); // wrapped() в ABI HanjiExecutor.
        assert.deepEqual(params[1],wrapped.params[1]);
        return word(getter);
      default: throw Error(method);
    }
  };
  return {request,hash,nonce};
}

test('Hanji accepts its shared executor for two wallets, rejecting another wrapped() with the same runtime',async()=>{
  const plan=await loadPlan(args,card);
  const rpc=replay(card.wrapped);
  assert.equal(await registeredHanji(rpc,plan,target),target);
  assert.equal(await registeredHanji(rpc,{...plan,wallet:market},target),target);
  const changed=replay(market);
  await assert.rejects(registeredHanji(changed,plan,target),/обёртку ETH/);
  await assert.rejects(deployed(changed,plan,changed.hash,null),/обёртку ETH/);
  await assert.rejects(deployed(rpc,{...plan,wallet:market},rpc.hash,null),/другой кошелёк/);
  assert.equal(addHanji(target,plan,target),target);
  assert.throws(()=>addHanji(market,plan,target));
});

test('A re-opened Hanji card resumes the original CREATE and preserves pending on a competing Venue write',async()=>{
  const plan=await loadPlan(args,card), rpc=replay(card.wrapped), saved=new Map();
  const storage={getItem:k=>saved.get(k),setItem:(k,v)=>saved.set(k,v),removeItem:k=>saved.delete(k)};
  const key=sourceDeploymentKey('hanji',venue,plan.chainId,'1',wallet,true);
  const selected=()=>({id:wallet,key,venue,kind:'hanji',settings:{...args,rows:null},router:null});
  deploymentScope(storage,selected).beforeOrAfterSend({chain:plan.chainId,from:wallet,nonce:rpc.nonce,creation:card.creationSha256});
  const otherKey=sourceDeploymentKey('hanji',venue,plan.chainId,'2',market,true);
  assert.equal(otherKey,key);
  const second=deploymentScope(storage,()=>({...selected(),id:market,key:otherKey}));
  assert.throws(()=>second.beforeOrAfterSend({from:market}),/ранее начатое/);
  const resumed=deploymentScope(storage,selected);
  const fact=await deployed(rpc,plan,rpc.hash,resumed.read());
  const next=addHanji(resumed.settings.rows,plan,fact.address);
  const writes=[];
  const save=async(...args)=>{writes.push(args); if(writes.length===1) throw Error('CAS conflict');};
  await assert.rejects(resumed.installSource('hanji.executor',next,save),/CAS conflict/);
  assert.equal(resumed.read().from,wallet);
  await resumed.installSource('hanji.executor',next,save);
  assert.deepEqual(writes[1],['hanji.executor',{kind:'Venue',id:venue},target,null]);
  resumed.save(null);
  assert.equal(second.read(),null);
});
