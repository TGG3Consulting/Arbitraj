// Архивный CREATE фиксирует единственный аргумент конструктора и код соответствующей фабрики.
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address=/^0x[0-9a-f]{40}$/;
const hash=/^0x[0-9a-f]{64}$/;
const same=(left,right)=>typeof left==='string' && left.toLowerCase()===right;
const chainByKind={empire_module:1,shibaswap_v2_module:1,registered_uni_v3:8453};
export const singleFactory=kind=>Object.hasOwn(chainByKind,kind);
export async function loadSingleFactory(settings,plan,sha){
  if(!singleFactory(settings.kind) || plan.kind!==settings.kind || plan.chainId!==chainByKind[settings.kind] ||
      String(settings.chainId)!==String(plan.chainId) || !address.test(plan.factory) || /^0x0+$/.test(plan.factory) ||
      !hash.test(plan.factoryHash) || !hash.test(plan.runtimeKeccak) || !hash.test(plan.sourceBlock) ||
      !same(settings.market,plan.factory) || !same(settings.factoryHash,plan.factoryHash) ||
      !settings.hashes?.some(h=>same(h,plan.runtimeKeccak))) throw Error('Профиль модуля и архив размещения расходятся.');
  if(!plan.data.endsWith(plan.factory.slice(2).padStart(64,'0')) || await sha(plan.data)!==plan.creationSha256)
    throw Error('Изменился код создания модуля или адрес фабрики.');
  return plan;
}

export async function checkSingleFactory(provider,plan,at){
  if(at?.requireCanonical!==true || !hash.test(at.blockHash)) throw Error('Нужен канонический блок выбранной сети.');
  const code=await provider.request({method:'eth_getCode',params:[plan.factory,at]});
  if(runtimeKeccak(code)!==plan.factoryHash) throw Error('Изменился проверенный код фабрики.');
}
