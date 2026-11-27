import {runtimeKeccak} from './uniswapV1Deploy.js';

const tupleRegistry = {llamma_lending:false, llamma_mint:false, llamma_v2:true};
const digest = /^0x[0-9a-f]{64}$/;
export const isLlamma = kind => Object.hasOwn(tupleRegistry,kind);

export async function loadLlamma(settings, plan, hashCode) {
  const factory = plan.roles?.factory;
  if (!isLlamma(plan.kind) || settings.kind !== plan.kind || plan.chainId !== 1 ||
      String(settings.chainId) !== '1' || plan.tupleRegistry !== tupleRegistry[plan.kind] ||
      !digest.test(plan.sourceBlock) || !digest.test(plan.runtimeKeccak) ||
      !settings.hashes?.some(hash => hash.toLowerCase() === plan.runtimeKeccak) ||
      !/^0x[0-9a-f]{40}$/.test(factory?.address) || /^0x0+$/.test(factory.address) ||
      settings.market?.toLowerCase() !== factory.address || !digest.test(factory.keccak256) ||
      settings.factoryHash?.toLowerCase() !== factory.keccak256) {
    throw Error('Профиль LLAMMA расходится с архивом фабрики или поколения.');
  }
  const args = factory.address.slice(2).padStart(64,'0') + Number(plan.tupleRegistry).toString().padStart(64,'0');
  if (!plan.data.endsWith(args) || await hashCode(plan.data) !== plan.creationSha256 ||
      !/^[0-9a-f]{64}$/.test(plan.runtimeSha256)) throw Error('Изменились код или параметры создания LlammaWallet.');
  return plan;
}

export async function checkLlamma(provider, plan, at) {
  if (at?.requireCanonical !== true || !digest.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  const factory = plan.roles.factory;
  const code = await provider.request({method:'eth_getCode', params:[factory.address,at]});
  if (runtimeKeccak(code) !== factory.keccak256) throw Error('Код фабрики LLAMMA изменился.');
}
