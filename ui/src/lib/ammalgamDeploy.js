// Профиль dLEx закрепляет CREATE, обвязку spot и обе записи прокси исходной пары.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const roles = ['factory','helper','pair','beacon','implementation'];
const digest = /^0x[0-9a-f]{64}$/;
const addr = /^0x[0-9a-f]{40}$/;
const matches = (a,b) => typeof a === 'string' && a.toLowerCase()===b;
const word = a => a.slice(2).padStart(64,'0');
const selector = signature => '0x'+sha3.keccak_256(signature).slice(0,8);
const slot = name => '0x'+(BigInt('0x'+sha3.keccak_256('eip1967.proxy.'+name))-1n).toString(16).padStart(64,'0');

export async function loadAmmalgam(s,p,sha) {
  if (p.kind!=='ammalgam_dlex' || s.kind!==p.kind || p.chainId!==1 || String(s.chainId)!=='1'
      || !digest.test(p.sourceBlock) || !digest.test(p.executorHash) || !digest.test(p.runtimeKeccak)
      || !/^[0-9a-f]{64}$/.test(p.runtimeSha256) || s.hashes?.length!==1 || !matches(s.hashes[0],p.runtimeKeccak)) {
    throw Error('Сеть или код модуля Ammalgam отличаются от проверенного профиля.');
  }
  const expected = [s.factoryHash,s.quoterHash,s.cloneHash,...(s.implementationHashes??[])];
  const addresses = [s.market,s.quoter,p.roles?.pair?.address,...(s.implementations??[])];
  if (addresses.length!==roles.length || expected.length!==roles.length
      || !roles.every((name,i)=>{
        const role=p.roles?.[name];
        return role && addr.test(role.address) && !/^0x0+$/.test(role.address) && digest.test(role.keccak256)
          && matches(addresses[i],role.address) && matches(expected[i],role.keccak256);
      }) || new Set(addresses.map(a=>a.toLowerCase())).size!==roles.length) {
    throw Error('Изменились фабрика, helper, эталонная пара, маяк или реализация Ammalgam.');
  }
  const args = roles.slice(0,3).map(name=>word(p.roles[name].address)).join('') + p.executorHash.slice(2);
  if (!p.data.endsWith(args) || await sha(p.data)!==p.creationSha256) throw Error('Изменились аргументы или байткод CREATE Ammalgam.');
  return p;
}

export async function checkAmmalgam(provider,p,at) {
  if (!at?.requireCanonical || !digest.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  const rpc=(method,params)=>provider.request({method,params});
  await Promise.all(roles.map(async name=>{
    const r=p.roles[name];
    if (runtimeKeccak(await rpc('eth_getCode',[r.address,at]))!==r.keccak256) throw Error('Изменился код Ammalgam: '+name);
  }));
  const r=p.roles;
  const getters=[[r.factory.address,'pairBeacon()',r.beacon.address],[r.beacon.address,'implementation()',r.implementation.address]];
  await Promise.all(getters.map(async ([to,signature,expected])=>{
    if (!matches(await rpc('eth_call',[{to,data:selector(signature)},at]),'0x'+word(expected))) throw Error('Изменилась роль Ammalgam: '+signature);
  }));
  await Promise.all(['beacon','implementation'].map(async name=>{
    const value=await rpc('eth_getStorageAt',[r.pair.address,slot(name),at]);
    if (!matches(value,'0x'+word(r[name].address))) throw Error('Эталонная пара Ammalgam сменила '+name);
  }));
}
