import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {checkMarket, deployed, hashCode, loadPlan, unsigned} from '../src/lib/walletDeployment.js';
import {runtimeKeccak} from '../src/lib/uniswapV1Deploy.js';

const read = (path) => JSON.parse(readFileSync(new URL(path,import.meta.url)));
const plan = read('../src/assets/uniswap-v1-ethereum.json');
const proof = read('../../tests/data/first_run_dex_2026-11-26/uniswap_v1_ethereum_prices.json');
const {factory, template, wrapped} = plan.roles;
const row = {kind:'uniswap_v1', chainId:1, market:factory.address, factoryHash:factory.keccak256,
  template:template.address, templateHash:template.keccak256, wrapped:wrapped.address,
  cloneHash:plan.cloneHash, hashes:[plan.runtimeKeccak]};
const word = (a) => '0x'+a.slice(2).padStart(64,'0');

test('Ethereum V1 CREATE binds both original constructor arguments and scoped settings', async()=>{
  assert.deepEqual(factory,proof.roles.factory);
  assert.deepEqual(template,proof.roles.template);
  assert.deepEqual(wrapped,proof.markets[proof.wrapped].token_code);
  assert.equal(plan.sourceBlock,proof.selector.blockHash);
  assert.equal(await loadPlan(row,plan),plan);
  assert.deepEqual(unsigned(plan),{chainId:'0x1',data:plan.data,value:'0x0'});
  for (const [field,value] of [['wrapped',factory.address],['chainId',8453],['templateHash',factory.keccak256],
    ['market',wrapped.address],['hashes',[]]]) {
    await assert.rejects(loadPlan({...row,[field]:value},plan),/настроек/);
  }
  const changed=plan.data.slice(0,-128)+word(wrapped.address).slice(2)+word(factory.address).slice(2);
  await assert.rejects(loadPlan(row,{...plan,data:changed,creationSha256:await hashCode(changed)}),/конструктора/);
});

test('Ethereum V1 recovery binds nonce, payer, chain, finalized inclusion and active template', async()=>{
  const payer='0x'+'18'.repeat(20), module='0x'+'19'.repeat(20), hash='0x'+'a4'.repeat(32);
  const block='0x'+'21'.repeat(32), final='0x'+'22'.repeat(32);
  const entries=[[factory,'0x01'],[template,'0x02'],[wrapped,'0x03']];
  const codes=new Map([...entries.map(([r,c])=>[r.address,c]),[module,'0x04']]);
  const roles={};
  for (const [name,role] of Object.entries(plan.roles)) {
    roles[name]={...role,sha256:await hashCode(codes.get(role.address)),keccak256:runtimeKeccak(codes.get(role.address))};
  }
  const p={...plan,roles,runtimeSha256:await hashCode('0x04'),runtimeKeccak:runtimeKeccak('0x04')};
  const tx={hash,to:null,from:payer,nonce:'0x9',input:p.data,blockHash:block,value:'0x0',chainId:'0x1'};
  const receipt={transactionHash:hash,contractAddress:module,blockHash:block,blockNumber:'0x10',status:'0x1'};
  const memo={chain:1,from:payer,nonce:'0x9',creation:p.creationSha256,hash};
  const visited=new Set();
  function rpc(change={}){return {async request({method,params}){
    if (method in change) return change[method](params);
    switch(method){
      case 'eth_chainId':return '0x1';
      case 'eth_getTransactionByHash':return tx;
      case 'eth_getTransactionReceipt':return receipt;
      case 'eth_getBlockByNumber':return params[0]==='finalized'?{number:'0x11',hash:final}:{number:'0x10',hash:block};
      case 'eth_getCode':
        assert.equal(params[1].requireCanonical,true); visited.add(params[1].blockHash);
        return codes.get(params[0]);
      case 'eth_call':
        assert.deepEqual(params[0],{to:factory.address,data:p.templateGetter});
        assert.equal(params[1].requireCanonical,true);
        return word(template.address);
      default:throw Error(method);
    }
  }};}
  assert.deepEqual(await deployed(rpc(),p,hash,memo),{address:module});
  assert.deepEqual(visited,new Set([block,final]));
  await assert.rejects(deployed(rpc(),p,hash,{...memo,nonce:'0xa'}),/nonce/);
  await assert.rejects(deployed(rpc({eth_requestAccounts:()=>[module]}),p,hash,null),/Плательщик/);
  await assert.rejects(deployed(rpc(),{...p,runtimeKeccak:runtimeKeccak('0x05')},hash,memo),/runtime/);
  await assert.rejects(deployed(rpc({eth_call:()=>word(wrapped.address)}),p,hash,memo),/шаблон/);
  await assert.rejects(checkMarket(rpc({eth_chainId:()=>'0x2105'}),p,{blockHash:final,requireCanonical:true}),/Ethereum/);
});
