// Проверяет фабрику, WETH и ABI поколения, из которых собран CREATE кошелька Mooniswap.
import {runtimeKeccak} from './uniswapV1Deploy.js';

const generations = {mooniswap_v1:false, mooniswap_v2:true};
export const mooniswap = kind => Object.hasOwn(generations, kind);
const digest = /^0x[0-9a-f]{64}$/;
const address = /^0x[0-9a-f]{40}$/;
const same = (a,b) => typeof a === 'string' && a.toLowerCase() === b;
const word = raw => raw.slice(2).padStart(64,'0');

export async function loadMooniswap(settings, plan, sha) {
  if (!mooniswap(settings.kind) || settings.kind !== plan.kind || String(settings.chainId) !== '1'
      || plan.chainId !== 1 || plan.hasSwapFor !== generations[plan.kind]
      || !digest.test(plan.sourceBlock) || !digest.test(plan.runtimeKeccak)
      || !settings.hashes?.some(hash => same(hash,plan.runtimeKeccak))) {
    throw Error('Сеть, вид пула или код модуля Mooniswap отличаются от архива.');
  }
  for (const [name,configured] of [['factory',settings.market],['wrapped',settings.wrapped]]) {
    const role = plan.roles?.[name];
    if (!role || !address.test(role.address) || /^0x0+$/.test(role.address)
        || !digest.test(role.keccak256) || !same(configured,role.address)) {
      throw Error('Проверьте фабрику и WETH выбранной площадки.');
    }
  }
  if (!same(settings.factoryHash,plan.roles.factory.keccak256) || plan.roles.factory.address === plan.roles.wrapped.address) {
    throw Error('Профиль Mooniswap расходится с кодом фабрики.');
  }
  const args = word(plan.roles.factory.address)+word(plan.roles.wrapped.address)+(plan.hasSwapFor?'1':'0').padStart(64,'0');
  if (!plan.data.endsWith(args) || await sha(plan.data)!==plan.creationSha256 || !/^[0-9a-f]{64}$/.test(plan.runtimeSha256)) {
    throw Error('Изменились данные создания контракта Mooniswap.');
  }
  return plan;
}

export async function checkCashRoles(provider, plan, at) {
  if (at?.requireCanonical !== true || !digest.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  await Promise.all(['factory','wrapped'].map(async name=>{
    const role = plan.roles[name];
    const code = await provider.request({method:'eth_getCode',params:[role.address,at]});
    if (runtimeKeccak(code)!==role.keccak256) throw Error('Изменился код контракта '+name+'.');
  }));
}

export const checkMooniswap = checkCashRoles;
