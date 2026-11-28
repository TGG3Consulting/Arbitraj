// FermiSlotExecutor использует константы ROUTER/ENGINE, общий адрес и отдельный учёт поручений каждого плательщика.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address = /^0x(?!0{40}$)[0-9a-f]{40}$/i;
const hash = /^0x(?!0{64}$)[0-9a-f]{64}$/i;
const word = a => '0x'+a.slice(2).toLowerCase().padStart(64,'0');
const request = (provider,method,params) => provider.request({method,params});
const getter = (provider,to,signature,at) => request(provider,'eth_call',[
  {to,data:'0x'+sha3.keccak_256(signature).slice(0,8)},at,
]);

export function fermiNetwork(venue,profiles,values) {
  const chain=values['dex.chain|Venue|'+venue], id=values['dex.evm_chain_id|Venue|'+venue];
  const profile=profiles?.find(p=>p.venue===venue) ?? profiles?.find(p=>p.evm_chain_id===id);
  return chain==='ethereum' && id===1 && profile?.chain===chain && profile.evm_chain_id===id ? id : null;
}

/** Проверяет именно настройки выбранной Ethereum-площадки и CREATE без аргументов. */
export async function loadFermi(settings,plan,digest) {
  const binds=settings.marketBindings;
  if (settings.kind!=='fermi' || plan.kind!==settings.kind || plan.chainId!==1 || settings.chainId!==1 ||
      !binds || !address.test(settings.wallet) || !address.test(plan.router) || !address.test(plan.engine) ||
      plan.router===plan.engine || ![plan.runtimeKeccak,plan.routerHash,plan.sourceBlock].every(v=>hash.test(v)) ||
      !/^[a-f0-9]{64}$/.test(plan.runtimeSha256) || await digest(plan.data)!==plan.creationSha256 ||
      ['router','engine','routerHash','runtimeKeccak'].some(key=>binds[key]?.toLowerCase()!==plan[key])) {
    throw Error('Сверьте сеть Ethereum, параметры Fermi и байткод создания модуля.');
  }
  return {...plan,wallet:settings.wallet.toLowerCase()};
}

export async function checkFermiMarket(provider,plan,at) {
  if (!at?.requireCanonical || !hash.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  const [code,engine]=await Promise.all([
    request(provider,'eth_getCode',[plan.router,at]),getter(provider,plan.router,'fermi()',at),
  ]);
  if (runtimeKeccak(code)!==plan.routerHash || engine?.toLowerCase()!==word(plan.engine)) {
    throw Error('Код роутера Fermi или его движок изменился.');
  }
}

export async function checkFermiModule(provider,plan,target,at) {
  const [router,engine]=await Promise.all([
    getter(provider,target,'ROUTER()',at),getter(provider,target,'ENGINE()',at),
  ]);
  if (router?.toLowerCase()!==word(plan.router) || engine?.toLowerCase()!==word(plan.engine)) {
    throw Error('Развёрнутый исполнитель Fermi привязан к другому рынку.');
  }
}

export async function registeredFermi(provider,plan,target) {
  if (target===null) return null;
  if (!address.test(target)) throw Error('Проверьте адрес fermi.executor.');
  if (BigInt(await request(provider,'eth_chainId',[]))!==1n) throw Error('Выберите Ethereum в кошельке.');
  const block=await request(provider,'eth_getBlockByNumber',['finalized',false]);
  const at={blockHash:block?.hash,requireCanonical:true};
  await checkFermiMarket(provider,plan,at);
  const code=await request(provider,'eth_getCode',[target,at]);
  if (runtimeKeccak(code)!==plan.runtimeKeccak) throw Error('Код fermi.executor отличается от проверенной сборки.');
  await checkFermiModule(provider,plan,target,at);
  return target;
}

export function addFermi(current,plan,target) {
  if (!address.test(target) || (current!==null && current.toLowerCase()!==target.toLowerCase())) {
    throw Error('У этой площадки уже задан другой fermi.executor.');
  }
  return current ?? target.toLowerCase();
}
