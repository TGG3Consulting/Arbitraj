// Общий NativeRfqAdapter для четырёх immutable: адреса и код официальных Router/RFQPool V6.
// Источник — изолированный CREATE в scripts/record_native_rfq_base.py и архивы выплат обеих сетей.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address=/^0x(?!0{40}$)[0-9a-f]{40}$/i, digest=/^0x[0-9a-f]{64}$/i;
const word=s=>s.slice(2).toLowerCase().padStart(64,'0');
const methods=['router','routerHash','pool','poolHash'];

const networks=new Map([[1,'ethereum'],[8453,'base']]);

export function nativeRfqNetwork(venue,profiles,values) {
  const id=values['dex.evm_chain_id|Venue|'+venue];
  const name=values['dex.chain|Venue|'+venue];
  const profile=profiles?.find(p=>p.venue===venue) ?? profiles?.find(p=>p.evm_chain_id===id);
  return profile && profile.evm_chain_id===id && profile.chain===name && networks.get(id)===name ? id : null;
}

function roles(rows,plan) {
  if (!Array.isArray(rows)) throw Error('Загрузите проверенные роли native_rfq.roles для выбранной сети.');
  const found=new Map();
  for (const line of rows) {
    if (typeof line!=='string') throw Error('Некорректная запись native_rfq.roles.');
    const [key,a,h,...more]=line.trim().split(/\s+/);
    if (more.length || found.has(key) || !['network','module','router','pool'].includes(key) ||
        (key==='network' ? a!==String(plan.chainId) || h!==networks.get(plan.chainId) : !address.test(a) || !digest.test(h))) {
      throw Error('Проверьте сеть и уникальность ролей Native RFQ.');
    }
    found.set(key,[a.toLowerCase(),h.toLowerCase()]);
  }
  if (!found.has('network') || ['router','pool'].some(k=>found.get(k)?.[0]!==plan[k] ||
      found.get(k)?.[1]!==plan[k+'Hash']) || (found.has('module') && found.get('module')[1]!==plan.runtimeKeccak)) {
    throw Error('Роли Native RFQ отличаются от подтверждённой карточки выбранной сети.');
  }
  return found.get('module')?.[0] ?? null;
}

export async function loadNativeRfq(settings,plan,sha) {
  if (settings.kind!=='native_rfq' || plan.kind!==settings.kind || !networks.has(plan.chainId) ||
      settings.chainId!==plan.chainId || !address.test(settings.wallet) || !digest.test(plan.sourceBlock) ||
      !digest.test(plan.runtimeKeccak) || !/^[0-9a-f]{64}$/.test(plan.runtimeSha256) ||
      ['router','pool'].some(k=>!address.test(plan[k]) || !digest.test(plan[k+'Hash'])) ||
      !plan.data.endsWith(methods.map(k=>word(plan[k])).join('')) || await sha(plan.data)!==plan.creationSha256) {
    throw Error('Проверьте сеть, плательщика и код создания Native RFQ.');
  }
  roles(settings.rows,plan);
  return {...plan,wallet:settings.wallet.toLowerCase()};
}

export async function checkNativeRoles(provider,plan,at) {
  if (at?.requireCanonical!==true || !digest.test(at.blockHash)) throw Error('Нужен канонический блок Native RFQ.');
  await Promise.all(['router','pool'].map(async k=>{
    const code=await provider.request({method:'eth_getCode',params:[plan[k],at]});
    if (runtimeKeccak(code)!==plan[k+'Hash']) throw Error('Изменился код '+k+' Native RFQ.');
  }));
  const data='0x'+sha3.keccak_256('isNativePools(address)').slice(0,8)+word(plan.pool);
  const listed=await provider.request({method:'eth_call',params:[{to:plan.router,data},at]});
  if(listed!=='0x'+'1'.padStart(64,'0')) throw Error('Router Native не зарегистрировал выбранный RFQPool.');
}
export async function checkNativeModule(provider,plan,target,at) {
  await Promise.all(methods.map(async k=>{
    const data='0x'+sha3.keccak_256(k+'()').slice(0,8);
    const got=await provider.request({method:'eth_call',params:[{to:target,data},at]});
    if (got?.toLowerCase()!=='0x'+word(plan[k])) throw Error('Экземпляр Native RFQ имеет другую роль '+k+'.');
  }));
}

export async function registeredNative(provider,plan,rows) {
  const target=roles(rows,plan);
  if (!target) return null;
  const id=await provider.request({method:'eth_chainId',params:[]});
  if (BigInt(id)!==BigInt(plan.chainId)) throw Error('Выберите сеть карточки в кошельке.');
  const head=await provider.request({method:'eth_getBlockByNumber',params:['finalized',false]});
  const at={blockHash:head?.hash,requireCanonical:true};
  await checkNativeRoles(provider,plan,at);
  const code=await provider.request({method:'eth_getCode',params:[target,at]});
  if (runtimeKeccak(code)!==plan.runtimeKeccak) throw Error('Изменился код общего модуля Native RFQ.');
  await checkNativeModule(provider,plan,target,at);
  return target;
}

export function addNative(rows,plan,target) {
  const old=roles(rows,plan);
  if (!address.test(target) || (old && old!==target.toLowerCase()) ||
      [plan.router,plan.pool].includes(target.toLowerCase())) throw Error('Конфликт адреса общего модуля Native RFQ.');
  return old ? rows : [...rows,'module '+target.toLowerCase()+' '+plan.runtimeKeccak];
}
