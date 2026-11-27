// Неподписанный CREATE одного EulerSwapWallet связывает обе фабрики выбранной сети.
import {runtimeKeccak} from './uniswapV1Deploy.js';

export const eulerSwap = kind => /^eulerswap_v[12](?:_module)?$/.test(kind);
const address = /^0x[0-9a-f]{40}$/;
const runtime = /^0x[0-9a-f]{64}$/;
const sha256 = /^[0-9a-f]{64}$/;
const same = (a,b) => typeof a==='string' && a.toLowerCase()===b;

export async function loadEulerSwap(settings, plan, digest) {
  if (plan.kind!=='eulerswap' || !eulerSwap(settings.kind) || ![1,8453].includes(plan.chainId) ||
      String(settings.chainId)!==String(plan.chainId) || !Array.isArray(plan.factories) ||
      plan.factories.length!==2 || !plan.factories.every(r=>r && address.test(r.address) && runtime.test(r.runtime)) ||
      plan.factories[0].address===plan.factories[1].address || !runtime.test(plan.runtimeKeccak) ||
      !sha256.test(plan.runtimeSha256) || !sha256.test(plan.creationSha256))
    throw Error('Архив размещения EulerSwap и выбранная сеть расходятся.');
  const role=plan.factories[settings.kind.startsWith('eulerswap_v2') ? 1 : 0];
  if (!same(settings.market,role.address) || !same(settings.factoryHash,role.runtime) ||
      !settings.hashes?.some(h=>same(h,plan.runtimeKeccak)))
    throw Error('Подтвердите фабрику и ожидаемый код модуля EulerSwap в настройках DEX.');
  const tail=plan.factories.map(f=>f.address.slice(2).padStart(64,'0')).join('');
  if (!plan.data.endsWith(tail) || await digest(plan.data)!==plan.creationSha256)
    throw Error('Код создания или адреса фабрик EulerSwap изменились.');
  return plan;
}

export async function checkEulerSwap(provider, plan, tag) {
  if (tag?.requireCanonical!==true || !runtime.test(tag.blockHash)) throw Error('Требуется канонический блок сети.');
  const codes=await Promise.all(plan.factories.map(f=>provider.request({method:'eth_getCode',params:[f.address,tag]})));
  if (!codes.every((code,i)=>runtimeKeccak(code)===plan.factories[i].runtime))
    throw Error('Код одной из фабрик EulerSwap изменился.');
}

