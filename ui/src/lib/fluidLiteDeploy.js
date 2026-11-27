import {runtimeKeccak} from './uniswapV1Deploy.js';
import {matchingCard} from './deploymentScope.js';

const officialResolver = '0x99b19a93ba0f516b173eef53d07b3253fbcfd564';
const hash = /^0x[\da-f]{64}$/;
const address = /^0x[\da-f]{40}$/;
const same = (value, expected) => typeof value === 'string' && value.toLowerCase() === expected;

export async function loadFluidLite(settings, plan, digest) {
  if (settings.kind !== 'fluid_dex_lite' || plan.kind !== settings.kind ||
      String(settings.chainId) !== '1' || plan.chainId !== 1 ||
      !hash.test(plan.sourceBlock) || !hash.test(plan.runtimeKeccak) ||
      !address.test(plan.exchange) || !address.test(plan.wrapped) ||
      !hash.test(plan.roles?.[plan.exchange]) || !hash.test(plan.roles?.[officialResolver]))
    throw Error('Не удалось подтвердить сеть или исходные контракты Fluid DEX Lite.');
  if (!same(settings.market, plan.exchange) || !same(settings.factoryHash, plan.roles[plan.exchange]) ||
      !same(settings.quoter, officialResolver) || !same(settings.quoterHash, plan.roles[officialResolver]) ||
      !same(settings.wrapped, plan.wrapped) || !settings.hashes?.some(h => same(h, plan.runtimeKeccak)))
    throw Error('Проверьте адреса и хеши кассы, квотера и модуля Fluid DEX Lite.');
  const args = [plan.exchange, plan.wrapped].map(a => a.slice(2).padStart(64, '0')).join('');
  if (!plan.data.endsWith(args) || await digest(plan.data) !== plan.creationSha256)
    throw Error('Параметры создания модуля Fluid DEX Lite отличаются от архивной проверки.');
  return plan;
}

export async function checkFluidLite(provider, plan, at) {
  if (at?.requireCanonical !== true || !hash.test(at.blockHash))
    throw Error('Для проверки Fluid DEX Lite требуется канонический блок Ethereum.');
  for (const target of [plan.exchange, officialResolver]) {
    const code = await provider.request({method:'eth_getCode', params:[target, at]});
    if (runtimeKeccak(code) !== plan.roles[target])
      throw Error('Изменился код контракта Fluid DEX Lite ' + target + '.');
  }
}

/** Точное исходное размещение имеет приоритет. Без поручения выбирается первая разрешённая версия. */
export function selectFluidCard(settings, pending, input, cards) {
  const card = matchingCard(settings, pending, input, cards);
  if (!card) throw Error('Для размещения Fluid DEX Lite требуется разрешённый хеш соответствующей версии.');
  return card;
}

/** Карточки из этой сборки. Обе затем проходят полную проверку loadFluidLite. */
export async function fluidCard(settings, pending, input) {
  const [{default: current}, {default: previous}] = await Promise.all([
    import('../assets/fluid-dex-lite-ethereum.json'),
    import('../assets/fluid-dex-lite-ethereum-previous.json'),
  ]);
  return { default: selectFluidCard(settings, pending, input, [current, previous]) };
}
