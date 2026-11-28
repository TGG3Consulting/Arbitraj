import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address=/^0x(?!0{40}$)[0-9a-f]{40}$/i;
const hash=/^0x[0-9a-f]{64}$/i;
const word=value=>value.slice(2).toLowerCase().padStart(64,'0');
const roles=['holder','holderHash','registry','registryHash'];
const implementationSlot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';

export async function loadZeroEx(settings,plan,sha) {
  if (settings.kind!=='zeroex' || plan.kind!==settings.kind || settings.chainId!==1 || plan.chainId!==1 ||
      !address.test(settings.wallet) || !hash.test(plan.runtimeKeccak) || !hash.test(plan.sourceBlock) ||
      ![plan.holder,plan.registry,plan.implementation].every(a=>address.test(a)) ||
      ![plan.holderHash,plan.registryHash,plan.implementationHash].every(a=>hash.test(a)) ||
      !plan.data.endsWith(roles.map(k=>word(plan[k])).join('')) || await sha(plan.data)!==plan.creationSha256) {
    throw Error('Проверьте кошелёк Ethereum и исходные контракты в карточке 0x.');
  }
  return {...plan,wallet:settings.wallet.toLowerCase()};
}
export async function checkZeroExRoles(provider,p,at) {
  if (!at?.requireCanonical || !hash.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  const keys=['holder','registry','implementation'];
  const [code,slot]=await Promise.all([
    Promise.all(keys.map(k=>provider.request({method:'eth_getCode',params:[p[k],at]}))),
    provider.request({method:'eth_getStorageAt',params:[p.registry,implementationSlot,at]})
  ]);
  if (keys.some((k,i)=>runtimeKeccak(code[i])!==p[k+'Hash']) || slot.toLowerCase()!=='0x'+word(p.implementation)) {
    throw Error('Изменились контракты расчёта или реализация реестра 0x.');
  }
}
export async function checkZeroExModule(provider,p,target,at) {
  const outputs=await Promise.all(roles.map(k=>provider.request({method:'eth_call',params:[
    {to:target,data:'0x'+sha3.keccak_256(k+'()').slice(0,8)},at]})));
  if (roles.some((k,i)=>outputs[i].toLowerCase()!=='0x'+word(p[k]))) throw Error('Размещённый модуль привязан к другим контрактам 0x.');
}
export function addZeroEx(current,p,target) {
  if (!address.test(target) || [p.holder,p.registry,p.implementation].includes(target.toLowerCase()) ||
      (current!=null && current.toLowerCase()!==target.toLowerCase())) throw Error('Проверьте адрес общего модуля 0x этой сети.');
  return target.toLowerCase();
}
export async function registeredZeroEx(provider,p,target) {
  if (target==null) return null;
  addZeroEx(null,p,target);
  if (BigInt(await provider.request({method:'eth_chainId',params:[]}))!==1n) throw Error('Выберите Ethereum в кошельке.');
  const block=await provider.request({method:'eth_getBlockByNumber',params:['finalized',false]});
  const at={blockHash:block?.hash,requireCanonical:true};
  await checkZeroExRoles(provider,p,at);
  const code=await provider.request({method:'eth_getCode',params:[target,at]});
  if (runtimeKeccak(code)!==p.runtimeKeccak) throw Error('Хеш модуля 0x отличается от карточки.');
  await checkZeroExModule(provider,p,target,at);
  return target;
}
