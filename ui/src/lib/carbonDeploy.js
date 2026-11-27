import {runtimeKeccak} from './uniswapV1Deploy.js';

const implementationSlot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
const digest=/^0x[0-9a-f]{64}$/;
const address=/^0x[0-9a-f]{40}$/;
const same=(a,b)=>typeof a==='string' && a.toLowerCase()===b;
const contains=(values,value)=>Array.isArray(values) && values.some(item=>same(item,value));

export async function loadCarbon(settings,p,hashCode) {
  if (p.kind!=='carbon_v1' || settings.kind!==p.kind || ![1,8453].includes(p.chainId) ||
      String(settings.chainId)!==String(p.chainId) || !address.test(p.market) || !address.test(p.implementation) ||
      /^0x0+$/.test(p.market) || /^0x0+$/.test(p.implementation) || p.market===p.implementation ||
      ![p.runtimeKeccak,p.sourceBlock,p.factoryHash,p.implementationHash].every(v=>digest.test(v)))
    throw Error('Проверьте сеть и адреса контроллера Carbon.');
  if (!same(settings.market,p.market) || !same(settings.factoryHash,p.factoryHash) ||
      !contains(settings.implementations,p.implementation) || !contains(settings.implementationHashes,p.implementationHash) ||
      !contains(settings.hashes,p.runtimeKeccak))
    throw Error('Контроллер, его реализация или код модуля отличаются от профиля Carbon.');
  if (await hashCode(p.data)!==p.creationSha256 || !p.data.endsWith(p.market.slice(2).padStart(64,'0')))
    throw Error('Код создания CarbonWallet или адрес контроллера изменился.');
  return p;
}

export async function checkCarbon(provider,p,at) {
  if (!at?.requireCanonical || !digest.test(at.blockHash)) throw Error('Для Carbon требуется канонический блок.');
  const call=(method,params)=>provider.request({method,params});
  const [proxy,slot,implementation]=await Promise.all([
    call('eth_getCode',[p.market,at]),call('eth_getStorageAt',[p.market,implementationSlot,at]),call('eth_getCode',[p.implementation,at]),
  ]);
  if (runtimeKeccak(proxy)!==p.factoryHash || runtimeKeccak(implementation)!==p.implementationHash ||
      !same(slot,'0x'+p.implementation.slice(2).padStart(64,'0'))) throw Error('Изменилась реализация контроллера Carbon.');
}
