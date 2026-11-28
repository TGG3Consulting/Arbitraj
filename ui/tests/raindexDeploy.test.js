// Размещение RaindexWallet из карточки Base: код и касса из записанных симуляций, восстановление браузера — модель.
// Вызывает node:test. Контракты из квитанций — изолированные адреса; рабочее размещение владельца не предполагается.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import sha3 from 'js-sha3';
import {deployed,loadPlan,sameRuntime,unsigned} from '../src/lib/walletDeployment.js';
import {addRaindex,registeredRaindex} from '../src/lib/raindexDeploy.js';
import {deploymentScope,sourceDeploymentKey} from '../src/lib/deploymentScope.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const file=path=>readFileSync(new URL(path,import.meta.url));
const record=name=>JSON.parse(gunzipSync(file('../../tests/data/first_run_dex_2026-11-26/'+name+'.json.gz')));
const card=JSON.parse(file('../src/assets/raindex-base-create.json'));
const quotes=record('raindex_base_quotes');
const funded=record('raindex_base_protected_wallets');
const codes=record('raindex_base_v6_codes');
const current=funded.modules[quotes.book], wallet=funded.payer, target=current.address;
const setting={kind:'raindex',chainId:funded.chain,book:quotes.book,wallet,rows:null};
const word=a=>'0x'+a.slice(2).padStart(64,'0');
const selector=name=>'0x'+sha3.keccak_256(name).slice(0,8);

test('Base V5 book keeps the source deployment height through another book registration',async()=>{
  const seed=record('raindex_base_v5_seed');
  const legacy=card.books.find(b=>b.book===seed.book);
  assert.equal(legacy.version,5);
  assert.equal(legacy.sourceKind,'rpc');
  assert.match(legacy.url,/^[1-9][0-9]*$/);
  assert.ok(BigInt(legacy.url)<BigInt(seed.block));
  const oldPlan=await loadPlan({...setting,book:legacy.book},card);
  // В этой проверке адреса строк фиктивные: проверяется сохранение настроек при добавлении книги.
  const first=addRaindex(null,oldPlan,'0x'+'ab'.repeat(20));
  const plan=await loadPlan({...setting,rows:first},card);
  const both=addRaindex(first,plan,target);
  assert.equal(both[0],first[0]);
  assert.ok(both[0].endsWith(' rpc '+legacy.url));
  assert.equal(both.length,2);
  for(const invalid of ['0','-1','1.5','18446744073709551616',quotes.subgraph]) {
    const rows=[first[0].replace('rpc '+legacy.url,'rpc '+invalid)];
    await assert.rejects(loadPlan({...setting,rows},card));
  }
});

test('Base archive binds the book, selector and paid order to the exact CREATE',async()=>{
  const plan=await loadPlan(setting,card);
  const args=word(quotes.book)+quotes.runtime.slice(2)+current.selector.padEnd(64,'0');
  assert.deepEqual(unsigned(plan),{chainId:'0x2105',value:'0x0',data:card.creation+args.slice(2)});
  assert.equal(plan.bookHash,runtimeKeccak(codes.bookCode));
  assert.equal(plan.sourceBlock,quotes.anchor.blockHash);
  assert.equal(codes.sourceBlock,plan.sourceBlock);
  assert.equal(codes.book,plan.book);
  assert.equal(plan.runtimeKeccak,current.runtime);
  assert.ok(await sameRuntime(codes.runtime,plan));
  assert.equal(plan.url,quotes.subgraph);
  const paid=funded.pairs.flatMap(p=>p.cases.flatMap(c=>Object.values(c))).find(r=>r?.stage==='paid' && r.book===plan.book);
  const settled=paid.logs.find(l=>l.address===target && l.topics[0]==='0x'+sha3.keccak_256('Settled(address,bytes32,address,address,uint256,uint256)'));
  assert.equal(settled.topics[1],word(wallet));
  assert.equal(settled.topics[2],runtimeKeccak(paid.action));
  assert.equal(paid.action.slice(0,10),plan.takeSelector);
  assert.equal(settled.topics[3],word(paid.pay));
  assert.equal(BigInt('0x'+settled.data.slice(66,130)),BigInt(paid.paid));
  assert.equal(BigInt('0x'+settled.data.slice(130,194)),BigInt(paid.net));
  await assert.rejects(loadPlan({...setting,chainId:1},card));
  await assert.rejects(loadPlan({...setting,book:target},card));
  await assert.rejects(loadPlan(setting,{...card,creation:card.creation+'00'}));
  await assert.rejects(loadPlan(setting,{...card,books:card.books.map(b=>({...b,takeSelector:'0xd0f7a862'}))}));
});

// Журнал браузера и финализированная квитанция CREATE моделируются. Ответы getCode соответствуют проверенному архиву.
function browser(plan,changed={}) {
  const hash=runtimeKeccak(plan.data), nonce='0x0', zero='0x'+'0'.repeat(64);
  const block={hash:card.sourceBlock,number:'0x'+quotes.height.toString(16),transactions:[hash]};
  const tx={hash,blockHash:block.hash,to:null,chainId:'0x2105',value:'0x0',input:plan.data,from:wallet,nonce,...changed.tx};
  const receipt={transactionHash:hash,contractAddress:target,blockHash:block.hash,blockNumber:block.number,status:'0x1'};
  const roles={
    [selector('book()')]:word(plan.book),[selector('bookCodeHash()')]:plan.bookHash,
    [selector('takeSelector()')]:plan.takeSelector.padEnd(66,'0'),...changed.roles,
  };
  return {hash,nonce,request:async({method,params})=>{
    switch(method) {
      case 'eth_chainId': return changed.chain??'0x2105';
      case 'eth_requestAccounts': return [wallet];
      case 'eth_getTransactionByHash': assert.equal(params[0],hash);return tx;
      case 'eth_getTransactionReceipt': assert.equal(params[0],hash);return receipt;
      case 'eth_getBlockByNumber': return block;
      case 'eth_getCode':
        assert.deepEqual(params[1],quotes.anchor);
        if(params[0]===plan.book) return changed.bookCode??codes.bookCode;
        assert.equal(params[0],target);return codes.runtime;
      case 'eth_getStorageAt': assert.deepEqual(params[2],quotes.anchor);return changed.slot??zero;
      case 'eth_call':
        assert.equal(params[0].to,target);assert.deepEqual(params[1],quotes.anchor);
        assert.ok(Object.hasOwn(roles,params[0].data));return roles[params[0].data];
      default: throw Error(method);
    }
  }};
}

test('A shared Raindex instance retains the operator source and rejects changed code or any immutable',async()=>{
  const plan=await loadPlan(setting,card);
  const rows=addRaindex(null,plan,target);
  assert.equal(rows[0],[card.chainId,plan.version,plan.book,plan.bookHash,target,plan.runtimeKeccak,'graphql',quotes.subgraph].join(' '));
  const configured=[rows[0].replace('graphql '+quotes.subgraph,'blockscout https://base.blockscout.com')];
  assert.strictEqual(addRaindex(configured,plan,target),configured);
  assert.equal(await registeredRaindex(browser(plan),plan,configured),target);
  // Модель второй строки профиля. Оплата доказана только для первой архивной V6.
  const otherBook={...plan,book:'0x2aee87d75cd000583daec7a28db103b1c0c18b76',version:3};
  const extended=addRaindex(configured,otherBook,wallet);
  assert.deepEqual(extended.slice(0,-1),configured);
  assert.notEqual(sourceDeploymentKey('raindex-'+otherBook.book,'base_dex',card.chainId,3,wallet,true),
    sourceDeploymentKey('raindex-'+plan.book,'base_dex',card.chainId,plan.version,wallet,true));
  for(const changed of [{bookCode:codes.runtime},{slot:word(target)},{chain:'0x1'},
    {roles:{[selector('book()')]:word(target)}},{roles:{[selector('bookCodeHash()')]:plan.runtimeKeccak}},
    {roles:{[selector('takeSelector()')]:word(plan.takeSelector)}}]) {
    await assert.rejects(registeredRaindex(browser(plan,changed),plan,configured));
  }
  await assert.rejects(loadPlan({...setting,rows:[...rows,...rows]},card));
  await assert.rejects(loadPlan({...setting,rows:[rows[0].replace(quotes.subgraph,quotes.subgraph+'?key=redacted')]},card));
  assert.throws(()=>addRaindex(rows,plan,wallet));
});

test('Reopened Base card binds the paid CREATE to the original payer and adds only its book with CAS',async()=>{
  const plan=await loadPlan(setting,card), rpc=browser(plan), venue='my_base',saved=new Map();
  const storage={getItem:k=>saved.get(k),setItem:(k,v)=>saved.set(k,v),removeItem:k=>saved.delete(k)};
  const key=sourceDeploymentKey('raindex-'+plan.book,venue,plan.chainId,plan.version,wallet,true);
  const selected=()=>({id:wallet,venue,key,kind:plan.kind,settings:setting});
  deploymentScope(storage,selected).beforeOrAfterSend({from:wallet,chain:plan.chainId,nonce:rpc.nonce,creation:plan.creationSha256});
  const original=deploymentScope(storage,selected);
  assert.throws(()=>original.beforeOrAfterSend({from:target}));
  await assert.rejects(deployed(browser(plan,{tx:{from:target}}),plan,rpc.hash,original.read()));
  await assert.rejects(deployed(browser(plan,{tx:{nonce:'0x1'}}),plan,rpc.hash,original.read()));
  const fact=await deployed(rpc,plan,rpc.hash,original.read());
  const next=addRaindex(original.settings.rows,plan,fact.address);
  await assert.rejects(original.installSource('raindex.books',next,async()=>{throw Error('CAS conflict');}),/CAS conflict/);
  assert.equal(original.read().from,wallet);
  const writes=[];
  await original.installSource('raindex.books',next,(...args)=>writes.push(args));
  assert.deepEqual(writes,[['raindex.books',{kind:'Venue',id:venue},next,null]]);
});
