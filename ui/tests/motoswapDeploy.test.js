import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {deployed,loadPlan,sameRuntime,unsigned} from '../src/lib/walletDeployment.js';
import {checkMotoModule,selectMotoCard} from '../src/lib/motoswapDeploy.js';

const read=file=>readFileSync(new URL(file,import.meta.url));
const profile=JSON.parse(read('../../config/first_run_dex.json')).find(p=>p.settings['dex.kind']==='motoswap_v2');
const card=name=>JSON.parse(read('../src/assets/'+name+'.json'));
const current=card('motoswap-ethereum');
const plan=card('motoswap-ethereum-previous');
const single=card('motoswap-ethereum-single');
const cards=[current,plan,single];
const archive=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/motoswap_v2_ethereum_wallets.json.gz')));
const active={kind:'motoswap_v2',chainId:profile.chain_id,market:profile.settings['dex.factory'],quoter:profile.settings['dex.quoter'],
  hashes:profile.settings['dex.module_runtime_hashes'],motoRoles:profile.settings['dex.motoswap_roles']};
const word=address=>'0x'+address.slice(2).padStart(64,'0');
test('Motoswap deploy reproduces the paid CREATE and binds both constructor roles',async()=>{
  const prepared=await loadPlan(active,plan);
  assert.equal(prepared.runtimeKeccak,archive.manifest.runtime);
  assert.deepEqual(unsigned(prepared),{chainId:'0x1',data:'0x'+archive.manifest.creation+word(active.market).slice(2)+word(active.quoter).slice(2),value:'0x0'});
  for (const change of [{kind:'mooniswap_v1'},{chainId:8453},{market:active.quoter},{hashes:[]},{motoRoles:active.motoRoles.slice(1)}])
    await assert.rejects(loadPlan({...active,...change},plan));
  await assert.rejects(loadPlan(active,{...plan,data:plan.data+'00'}));
  const tag={blockHash:plan.sourceBlock,requireCanonical:true};
  const provider={request:async({method,params:[call,at]})=>{
    assert.equal(method,'eth_call');assert.deepEqual(at,tag);assert.equal(call.to,archive.module);
    return word(call.data==='0xc45a0155'?active.market:active.quoter);
  }};
  await checkMotoModule(provider,prepared,archive.module,tag);
  await assert.rejects(checkMotoModule({request:async()=>word(active.quoter)},prepared,archive.module,tag));
});

const evidence=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/motoswap_creation_v3.json.gz')));
const snapshot=JSON.parse(gunzipSync(read('../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz')));
const versions=[[current,evidence.currentRuntime],[plan,evidence.previousRuntime],[single,evidence.singleRuntime]];
const links={
  factory:{'0x59659e90':'beacon','0x5c60da1b':'pair_impl','0xa0e5a3e2':'registry'},
  fee_router:{'0xf887ea40':'core','0x7b103999':'registry','0x9a6a5cf0':'creator','0x29269144':'vault','0x913e77ad':'collector'},
  core:{'0xc45a0155':'factory'},beacon:{'0x5c60da1b':'pair_impl'},
  module:{'0xc45a0155':'factory','0xf887ea40':'fee_router'},
};
// CREATE проверялся через eth_call. Для проверки восстановления моделируем только
// конверт включения и ответ кошелька. Код, адреса, calldata и связи ролей взяты из архива.
const hash='0x'+'ac'.repeat(32), nonce='0x1';
function provider(manifest,code,account=evidence.owner){
  const at={blockHash:current.sourceBlock,requireCanonical:true};
  const height='0x'+snapshot.block.toString(16);
  const roles={...snapshot.directory.roles,module:archive.module};
  const byAddress=Object.fromEntries(Object.entries(roles).map(([name,target])=>[target,name]));
  const proxy={factory:'factory_impl',fee_router:'fee_impl',collector:'collector_impl'};
  return {request:async({method,params})=>{
    switch(method){
    case 'eth_chainId':return '0x1';
    case 'eth_requestAccounts':return [account];
    case 'eth_getTransactionByHash':
      assert.deepEqual(params,[hash]);
      return {from:evidence.owner,to:null,hash,nonce,value:'0x0',chainId:'0x1',blockHash:at.blockHash,input:manifest.data};
    case 'eth_getTransactionReceipt':
      assert.deepEqual(params,[hash]);
      return {transactionHash:hash,blockHash:at.blockHash,blockNumber:height,status:'0x1',contractAddress:archive.module};
    case 'eth_getBlockByNumber':
      assert.ok(params[0]==='finalized' || params[0]===height);
      return {number:height,hash:at.blockHash,transactions:[hash]};
    case 'eth_getCode':
      assert.deepEqual(params[1],at);
      return params[0]===archive.module ? code : evidence.roles[params[0]];
    case 'eth_getStorageAt':{
      assert.deepEqual(params[2],at);
      assert.equal(params[1],'0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc');
      const impl=proxy[byAddress[params[0]]];
      return impl?word(roles[impl]):'0x'+'00'.repeat(32);
    }
    case 'eth_call':{
      assert.deepEqual(params[1],at);
      const {to,data}=params[0];
      const target=links[byAddress[to]]?.[data];
      assert.ok(target);
      return word(roles[target]);
    }
    default:throw Error('Неожиданный вызов '+method);
    }
  }};
}

test('Motoswap verifies finalized and pending CREATE for each permitted runtime',async()=>{
  assert.equal(new Set(cards.map(c=>c.creationSha256)).size,3);
  assert.equal(selectMotoCard(active,null,undefined,cards),current);
  for(const [c,code] of versions){
    const pending={chain:1,from:evidence.owner,nonce,creation:c.creationSha256,hash};
    assert.equal(c.sourceBlock,evidence.verification.sourceBlock);
    assert.equal(await sameRuntime(code,c),true);
    assert.equal(selectMotoCard({...active,hashes:[c.runtimeKeccak]},null,undefined,cards),c);
    for(const intent of [pending,null]){
      const chosen=selectMotoCard(active,intent,intent?undefined:c.data,cards);
      assert.equal(chosen,c);
      assert.deepEqual(await deployed(provider(c,code),await loadPlan(active,chosen),hash,intent),{address:archive.module});
    }
    assert.throws(()=>selectMotoCard({...active,hashes:[]},pending,undefined,cards));
    assert.throws(()=>selectMotoCard(active,{...pending,chain:8453},undefined,cards));
    assert.throws(()=>selectMotoCard(active,{...pending,creation:c.runtimeSha256},undefined,cards));
  }
  const previous=await loadPlan(active,plan), rpc=provider(plan,evidence.previousRuntime);
  const pending={chain:1,from:evidence.owner,nonce,creation:plan.creationSha256,hash};
  await assert.rejects(deployed(rpc,previous,hash,{...pending,nonce:'0x2'}));
  await assert.rejects(deployed(provider(plan,evidence.previousRuntime,active.market),previous,hash,null));
  await assert.rejects(deployed(provider(plan,evidence.currentRuntime),previous,hash,pending));
  assert.throws(()=>selectMotoCard(active,null,plan.data+'00',cards));
  assert.throws(()=>selectMotoCard(active,pending,current.data,cards));
});

