import {runtimeKeccak} from './uniswapV1Deploy.js';

const address = /^0x[0-9a-f]{40}$/;
const digest = /^0x[0-9a-f]{64}$/;
const same = (a,b) => typeof a === 'string' && a.toLowerCase() === b;
const argument = a => a.slice(2).padStart(64,'0');

const factories = new Map([['balancer_v1','Balancer V1'],['yield_basis','YieldBasis'],['bancor_legacy','Bancor V2.1']]);

export const supportsFactoryWallet = kind => factories.has(kind);

export async function loadFactoryWallet(settings, plan, hashCode) {
  const label = factories.get(plan.kind);
  if (!label || plan.chainId !== 1 ||
      ![plan.factory,plan.wrapped].every(a => address.test(a) && !/^0x0+$/.test(a)) || plan.factory === plan.wrapped ||
      ![plan.factoryHash,plan.wrappedHash,plan.runtimeKeccak,plan.sourceBlock].every(h => digest.test(h)))
    throw Error('Формат архива модуля Ethereum не распознан.');
  if (settings.kind !== plan.kind || String(settings.chainId) !== '1' || !same(settings.market,plan.factory) ||
      !same(settings.factoryHash,plan.factoryHash) || !same(settings.wrapped,plan.wrapped) ||
      !settings.hashes?.some(h => same(h,plan.runtimeKeccak)))
    throw Error('Сеть, фабрика, WETH или допустимый код отличаются от профиля '+label+'.');
  const legacy = plan.kind==='bancor_legacy';
  if (legacy && (!address.test(plan.network) || /^0x0+$/.test(plan.network) || [plan.factory,plan.wrapped].includes(plan.network) ||
      !digest.test(plan.networkHash) || !same(settings.quoter,plan.network) || !same(settings.quoterHash,plan.networkHash)))
    throw Error('Сеть Bancor и её проверенный код отличаются от профиля.');
  const args=legacy ? [plan.factory,plan.network,plan.wrapped] : [plan.factory,plan.wrapped];
  if (await hashCode(plan.data) !== plan.creationSha256 || !plan.data.endsWith(args.map(argument).join('')))
    throw Error('Код создания или параметры конструктора '+label+' изменились.');
  return plan;
}

export async function checkFactoryWallet(provider, p, at) {
  if (at?.requireCanonical !== true || !digest.test(at.blockHash)) throw Error('Требуется канонический блок Ethereum.');
  const roles=[['Фабрика '+factories.get(p.kind),p.factory,p.factoryHash],['WETH',p.wrapped,p.wrappedHash]];
  if (p.kind==='bancor_legacy') roles.push(['BancorNetwork',p.network,p.networkHash]);
  for (const [role,addr,hash] of roles) {
    const code = await provider.request({method:'eth_getCode',params:[addr,at]});
    if (runtimeKeccak(code) !== hash) throw Error('Код '+role+' отличается от проверенного на Ethereum.');
  }
}
