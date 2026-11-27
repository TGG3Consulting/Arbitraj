import test from 'node:test';
import assert from 'node:assert/strict';
import {deployed,hashCode} from '../src/lib/walletDeployment.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';
const hash = i => '0x'+i.repeat(64);
const addr = i => '0x'+i.repeat(40);
const word = a => '0x'+a.slice(2).padStart(64,'0');
const payer=addr('1'), module=addr('2'), original=hash('a'), replacement=hash('b');
const block=hash('c'),final=hash('d'),code='0x6080';
async function setup(kind, changed={}) {
  const ks=await hashCode(code), kp=runtimeKeccak(code);
  const role=a=>({address:addr(a),sha256:ks,keccak256:kp,implementation:{address:addr('f'),keccak256:kp}});
  const names=['BancorNetwork','BancorNetworkInfo','MasterVault','BNTPool','NetworkSettings'];
  const roles=kind==='uniswap_v1'
    ? {factory:role('3'),template:role('4'),wrapped:role('5')}
    : Object.fromEntries(names.map((n,i)=>[n,role(String(i+3))]));
  const chainId=kind==='stabull'?8453:1;
  const rpcChain='0x'+chainId.toString(16);
  const p={kind,chainId,market:addr('3'),factoryHash:kp,data:'0x6000',roles,runtimeKeccak:kp,runtimeSha256:ks,templateGetter:'0x01',
    arguments:[addr('3'),addr('4'),addr('5'),addr('8'),addr('9')],wrappedHash:kp};
  p.creationSha256=await hashCode(p.data);
  const pending={from:payer,nonce:'0x7',creation:p.creationSha256,chain:chainId,hash:original};
  const tx={hash:replacement,to:null,from:payer,nonce:'0x7',input:p.data,value:'0x0',blockHash:block,chainId:rpcChain,...changed.tx};
  const receipt={transactionHash:replacement,status:'0x1',contractAddress:module,blockNumber:'0x41',blockHash:block,...changed.receipt};
  const rpc={async request({method,params}) {
    switch(method){
      case 'eth_chainId':return rpcChain;
      case 'eth_getTransactionByHash':return tx;
      case 'eth_getTransactionReceipt':return receipt;
      case 'eth_getBlockByNumber':return params[0]==='finalized'?{number:changed.finalHeight??'0x42',hash:final}:
        {number:'0x41',hash:changed.blockHash??block,transactions:changed.hashes??[replacement]};
      case 'eth_getCode':return params[0]===addr('9')?'0x':code;
      case 'eth_getStorageAt':return word(addr('f'));
      case 'eth_call':{
        const selectors={'0x6739afca':'3','0x3b7e3775':'5','0xcdab39ce':'6','0x8bcab632':'7'};
        return word(addr(selectors[params[0].data]??'4'));
      }
      default:throw Error('Unexpected RPC '+method);
    }
  }};
  return {p,pending,rpc};
}

for(const kind of ['uniswap_v1','bancor_v3','stabull']) {
  test(kind+' repriced CREATE is bound to the original nonce, signed data, finalized inclusion and runtime',async()=>{
    const {rpc,p,pending}=await setup(kind);
    assert.deepEqual(await deployed(rpc,p,replacement,pending),{address:module,replacementHash:replacement});
    for(const changed of [{tx:{nonce:'0x8'}},{tx:{from:addr('6')}},{tx:{chainId:'0x2'}},{hashes:[original]},
      {blockHash:hash('e')},{finalHeight:'0x40'}]){
      const failed=await setup(kind,changed);
      await assert.rejects(deployed(failed.rpc,failed.p,replacement,failed.pending));
    }
  });
  test(kind+' cancellation consumes exactly one finalized nonce, including a lost original wallet reply',async()=>{
    const {rpc,p,pending}=await setup(kind,{tx:{to:payer,input:'0x'},receipt:{contractAddress:null}});
    const noReply={...pending,hash:undefined};
    const result=await deployed(rpc,p,replacement,noReply);
    assert.deepEqual(result,{consumed:true,earlierCreation:false,succeeded:true,hash:replacement,originalHash:null,
      blockHash:block,from:payer,nonce:'0x7'});
    await assert.rejects(deployed(rpc,p,replacement,null));
  });
  test(kind+' finalized old-manifest creation does not install a newer module',async()=>{
    const previous='0x6001';
    const {rpc,p,pending}=await setup(kind,{tx:{input:previous}});
    const result=await deployed(rpc,p,replacement,{...pending,creation:await hashCode(previous)});
    assert.equal(result.consumed,true);assert.equal(result.earlierCreation,true);assert.equal(result.address,undefined);
  });
}
