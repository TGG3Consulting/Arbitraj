// Карточка общего HanjiExecutor. Экран кошелька проверяет CREATE из Actions и wrapped() экземпляра.
// Источники: contracts/HanjiExecutor.sol и tests/data/first_run_dex_2026-11-26/hanji_executor_receipts.json.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address = /^0x(?!0{40}$)[0-9a-f]{40}$/i;
const digest = /^0x[0-9a-f]{64}$/;
const word = value => value.slice(2).toLowerCase().padStart(64,'0');
const wrappedGetter = '0x'+sha3.keccak_256('wrapped()').slice(0,8);

export {baseNetwork as hanjiNetwork} from './baseNetwork.js';

/** Возвращает проверенный CREATE одного shared-модуля с аргументом WETH и выбранным плательщиком. */
export async function loadHanji(settings,plan,sha) {
  if (settings.kind!=='hanji' || plan.kind!==settings.kind || plan.chainId!==8453 || settings.chainId!==plan.chainId ||
      !address.test(settings.wallet) || !address.test(plan.wrapped) || settings.wrapped?.toLowerCase()!==plan.wrapped ||
      !digest.test(plan.sourceBlock) || !digest.test(plan.wrappedHash) || !digest.test(plan.runtimeKeccak) || !/^[0-9a-f]{64}$/.test(plan.runtimeSha256) ||
      !plan.data.endsWith(word(plan.wrapped)) || await sha(plan.data)!==plan.creationSha256) {
    throw Error('Проверьте сеть, адрес плательщика, WETH и исходный код создания Hanji.');
  }
  return {...plan,wallet:settings.wallet.toLowerCase()};
}

/** Проверяет код обёртки на конкретном каноническом блоке перед CREATE и при восстановлении. */
export async function checkHanjiWrapped(provider,plan,at) {
  if (at?.requireCanonical!==true || !digest.test(at.blockHash)) throw Error('Нужен канонический блок Base.');
  const code = await provider.request({method:'eth_getCode',params:[plan.wrapped,at]});
  if (runtimeKeccak(code)!==plan.wrappedHash) throw Error('Код WETH на Base отличается от архива Hanji.');
}

/** wrapped хранится в storage. Его нужно сверять отдельно даже после совпадения runtime. */
export async function checkHanjiModule(provider,plan,target,at) {
  const result = await provider.request({method:'eth_call',params:[{to:target,data:wrappedGetter},at]});
  if (result?.toLowerCase()!=='0x'+word(plan.wrapped)) throw Error('Экземпляр Hanji использует другую обёртку ETH.');
}

/** Возвращает общий действующий адрес сети. Ошибка действующей настройки требует проверки до нового CREATE. */
export async function registeredHanji(provider,plan,target) {
  if (target===null) return null;
  if (!address.test(target)) throw Error('Проверьте заданный адрес hanji.executor.');
  const chain = await provider.request({method:'eth_chainId',params:[]});
  if (BigInt(chain)!==BigInt(plan.chainId)) throw Error('Выберите Base в кошельке.');
  const block = await provider.request({method:'eth_getBlockByNumber',params:['finalized',false]});
  const at = {blockHash:block?.hash,requireCanonical:true};
  await checkHanjiWrapped(provider,plan,at);
  const code = await provider.request({method:'eth_getCode',params:[target,at]});
  if (runtimeKeccak(code)!==plan.runtimeKeccak) throw Error('Код настроенного Hanji отличается от принятого.');
  await checkHanjiModule(provider,plan,target,at);
  return target;
}

/** Готовит единственный адрес для Venue. Повтор по тому же адресу не меняет запись. */
export function addHanji(current,plan,target) {
  if (!address.test(target) || (current!==null && current.toLowerCase()!==target.toLowerCase())) {
    throw Error('У этой площадки уже задан другой адрес hanji.executor.');
  }
  return current ?? target.toLowerCase();
}
