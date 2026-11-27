// Проверка ABI, по которому Rust допускает executePath. CI запускает статический вызов
// в локальной EVM на байтах, полученных из каждого конструктора на архивном блоке.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {Interface,getBytes,hexlify,keccak256} from 'ethers';
import {addr,chain} from './lib.mjs';

const fixture=name=>JSON.parse(gunzipSync(readFileSync(new URL(
  '../../tests/data/first_run_dex_2026-11-26/'+name+'.json.gz',import.meta.url))));
const evidence=fixture('motoswap_creation_v3');
const {module,owner}=fixture('motoswap_v2_ethereum_wallets');
const abi=new Interface(['function pathVersion() pure returns(uint256)']);
const data=getBytes(abi.encodeFunctionData('pathVersion'));
for(const [file,code,hasPath] of [
  ['motoswap-ethereum-single',evidence.singleRuntime,false],
  ['motoswap-ethereum-previous',evidence.previousRuntime,true],
  ['motoswap-ethereum',evidence.currentRuntime,true],
]){
  const card=JSON.parse(readFileSync(new URL('../../ui/src/assets/'+file+'.json',import.meta.url)));
  assert.equal(keccak256(code),card.runtimeKeccak);
  assert.equal(card.sourceBlock,evidence.verification.sourceBlock);
  const {evm,sm}=await chain();
  await sm.putContractCode(addr(module),getBytes(code));
  const {execResult}=await evm.runCall({caller:addr(owner),origin:addr(owner),to:addr(module),data,isStatic:true});
  if(hasPath){
    assert.ifError(execResult.exceptionError);
    assert.equal(hexlify(execResult.returnValue),abi.encodeFunctionResult('pathVersion',[1n]));
  }else{
    assert.equal(execResult.exceptionError?.error,'revert');
  }
}
