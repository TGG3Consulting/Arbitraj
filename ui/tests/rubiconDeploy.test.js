// Checks use only a local RPC double. The bundled CREATE bytes are produced by the Base archive job.
import {readFileSync} from 'node:fs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {checkMarket, deployed, hashCode, loadPlan, send, unsigned} from '../src/lib/walletDeployment.js';

const archived = JSON.parse(readFileSync(new URL('../public/rubicon-classic-base.json',import.meta.url)));
const settings={kind:'rubicon_classic', market:archived.market, hashes:[archived.runtimeKeccak],
  implementations:[archived.implementation]};

test('the confirmed constructor data binds the admitted market and code',async()=>{
  assert.equal(await loadPlan(settings,archived),archived);
  assert.deepEqual(unsigned(archived),{chainId:'0x2105',data:archived.data,value:'0x0'});
  await assert.rejects(loadPlan({...settings,implementations:[]},archived),/реализация/);
  await assert.rejects(loadPlan({...settings,hashes:[]},archived),/код/);
  await assert.rejects(loadPlan(settings,{...archived,data:archived.data.replace(/^0x../,'0xff')}),/Байткод/);
});

const hash='0x'+'11'.repeat(32), block='0x'+'22'.repeat(32), final='0x'+'33'.repeat(32);
const implementation='0x'+'ab'.repeat(20), market='0x'+'cd'.repeat(20), module='0x'+'ef'.repeat(20);
const payer='0x'+'34'.repeat(20);
async function environment(changes = {}){
  const p={...archived,market,implementation,proxySha256:await hashCode('0x01'),
    implementationSha256:await hashCode('0x02'),runtimeSha256:await hashCode('0x03')};
  const tx={hash,from:payer,to:null,chainId:'0x2105',input:p.data,value:'0x0',nonce:'0x9',blockHash:block};
  const receipt={transactionHash:hash,blockHash:block,blockNumber:'0x10',contractAddress:module,status:'0x1'};
  const provider={async request({method,params}){
    if (Object.hasOwn(changes,method)) return changes[method](params);
    switch(method){
      case 'eth_chainId':return '0x2105';
      case 'eth_getTransactionByHash':return tx;
      case 'eth_getTransactionReceipt':return receipt;
      case 'eth_getBlockByNumber':return params[0]==='finalized'?{number:'0x11',hash:final}:{number:'0x10',hash:block};
      case 'eth_getStorageAt':return '0x'+implementation.slice(2).padStart(64,'0');
      case 'eth_getCode':
        assert.deepEqual(params[1],{blockHash:final,requireCanonical:true});
        return new Map([[market,'0x01'],[implementation,'0x02'],[module,'0x03']]).get(params[0]);
      default: throw Error(method);
    }
  }};
  return {provider,p};
}

test('a matching finalized CREATE is required for publication after restart',async()=>{
  const {provider,p}=await environment();
  const pending={from:payer,nonce:'0x9',creation:p.creationSha256,chain:8453};
  assert.deepEqual(await deployed(provider,p,hash,pending),{address:module});
  await assert.rejects(deployed(provider,p,hash,{...pending,nonce:'0xa'}),/nonce/);
  const orphan=await environment({eth_getBlockByNumber:()=>({hash:final,number:'0x11'})});
  await assert.rejects(deployed(orphan.provider,orphan.p,hash,pending),/финализированного/);
  const wrong=await environment({eth_chainId:()=>'0x1'});
  await assert.rejects(checkMarket(wrong.provider,p,{}),/Base/);
});

test('an accepted dispatch with a lost reply retains the original nonce',async()=>{
  for (const [code,cleared] of [[-32005,false],[4001,true]]) {
    const records=[], signed=[];
    const {provider,p}=await environment({
      eth_getBlockByNumber:()=>({hash:final,number:'0x11'}),
      eth_requestAccounts:()=>[payer], eth_estimateGas:()=> '0x50000',eth_gasPrice:()=> '0x2',
      eth_call:()=> '0x03',eth_getTransactionCount:()=> '0x9',
      eth_sendTransaction:([request])=>{signed.push(request);throw Object.assign(Error('wallet reply'),{code});}
    });
    await assert.rejects(send(provider,p,{from:payer,fee:String(0x50000*2)},v=>records.push(v)),/wallet reply/);
    assert.equal(signed.length,1);
    assert.equal(records[0].nonce,'0x9');
    assert.equal(records[0].creation,p.creationSha256);
    assert.equal(records.at(-1)===null,cleared);
  }
});
