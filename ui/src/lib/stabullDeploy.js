// Проверка неподписанного CREATE и единственной immutable роли Stabull на Base.
import {runtimeKeccak} from './uniswapV1Deploy.js';

const digest = /^0x[0-9a-f]{64}$/;
const sha = /^[0-9a-f]{64}$/;
const address = /^0x[0-9a-f]{40}$/;
const same = (x, expected) => typeof x === 'string' && x.toLowerCase() === expected;
export async function loadStabull(settings, plan, hashCode) {
  if (plan.kind !== 'stabull' || plan.chainId !== 8453 || !address.test(plan.market) ||
      !digest.test(plan.factoryHash) || !digest.test(plan.runtimeKeccak) ||
      !sha.test(plan.creationSha256) || !sha.test(plan.runtimeSha256))
    throw Error('Архив размещения Stabull на Base не распознан.');
  if (!['stabull','stabull_module'].includes(settings.kind) || String(settings.chainId) !== '8453' ||
      !same(settings.market, plan.market) || !same(settings.factoryHash, plan.factoryHash) ||
      !settings.hashes?.some(h => same(h, plan.runtimeKeccak)))
    throw Error('Сеть, фабрика или допустимый runtime Stabull в настройках отличается от архива.');
  if (await hashCode(plan.data) !== plan.creationSha256 || !plan.data.toLowerCase().endsWith(plan.market.slice(2).padStart(64,'0')))
    throw Error('Код создания или аргумент фабрики Stabull изменился.');
  return plan;
}

export async function checkStabull(provider, plan, at) {
  if (at?.requireCanonical !== true || !digest.test(at.blockHash)) throw Error('Требуется канонический блок Base.');
  const code = await provider.request({method:'eth_getCode', params:[plan.market,at]});
  if (runtimeKeccak(code) !== plan.factoryHash) throw Error('Код фабрики Stabull в сети изменился.');
}
