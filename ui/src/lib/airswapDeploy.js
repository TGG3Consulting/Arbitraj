import {runtimeKeccak} from './uniswapV1Deploy.js';

const digest = /^0x[0-9a-f]{64}$/;
const address = /^0x[0-9a-f]{40}$/;
export const isAirSwap = kind => ['airswap_v4','airswap_v5'].includes(kind);
const fields = line => typeof line === 'string' ? line.trim().toLowerCase().split(/\s+/) : [];
const matches = (p,row) => row[0]===String(p.chainId) && row[1]===p.kind.slice(-2) &&
  row[2]===p.exchange && row[3]===p.exchangeHash;

export async function loadAirSwap(settings,p,hashCode) {
  const wallet=settings.wallet?.toLowerCase();
  if (!isAirSwap(p.kind) || p.kind!==settings.kind || ![1,8453].includes(p.chainId) ||
      String(settings.chainId)!==String(p.chainId) || !address.test(wallet) || /^0x0+$/.test(wallet) ||
      !address.test(p.exchange) || !digest.test(p.exchangeHash) || !digest.test(p.runtimeKeccak) ||
      !digest.test(p.sourceBlock) || !/^[0-9a-f]{64}$/.test(p.runtimeSha256) ||
      !settings.rows?.some(line=>matches(p,fields(line))) ||
      !p.data.endsWith(p.exchange.slice(2).padStart(64,'0')) || await hashCode(p.data)!==p.creationSha256) {
    throw Error('Проверьте сеть, кошелёк и адрес с хешем AirSwap в airswap.contracts.');
  }
  return {...p,wallet};
}

export async function checkExchange(provider,p,at) {
  if (at?.requireCanonical!==true || !digest.test(at.blockHash)) throw Error('Нужен канонический блок AirSwap.');
  const code=await provider.request({method:'eth_getCode',params:[p.exchange,at]});
  if (runtimeKeccak(code)!==p.exchangeHash) throw Error('Код обменного контракта AirSwap изменился.');
}

// Every wallet can register its own module for the same settlement contract.
export async function registeredOwnerModule(provider,p,rows) {
  const candidates = rows.map(fields).filter(row =>
    matches(p,row) && row.length===6 && address.test(row[4]) && row[5]===p.runtimeKeccak);
  if (!candidates.length) return null;
  const chain = await provider.request({method:'eth_chainId',params:[]});
  if (BigInt(chain)!==BigInt(p.chainId)) throw Error('Выберите сеть модуля AirSwap в кошельке.');
  const head=await provider.request({method:'eth_getBlockByNumber',params:['latest',false]});
  if (!digest.test(head?.hash)) throw Error('Не удалось проверить блок размещения AirSwap.');
  const at={blockHash:head.hash,requireCanonical:true};
  for (const target of new Set(candidates.map(row=>row[4]))) {
    const code=await provider.request({method:'eth_getCode',params:[target,at]});
    if (code==='0x' || runtimeKeccak(code)!==p.runtimeKeccak) continue;
    const owner=await readAirSwapOwner(provider,target,at);
    if (owner===p.wallet) return target;
  }
  return null;
}

async function readAirSwapOwner(provider,target,at) {
  const result=await provider.request({method:'eth_call',params:[{to:target,data:'0x8da5cb5b'},at]});
  if (!/^0x0{24}[0-9a-f]{40}$/i.test(result)) throw Error('Не удалось проверить владельца модуля AirSwap.');
  return '0x'+result.slice(-40).toLowerCase();
}

export async function checkAirSwapOwner(provider,p,target,at) {
  if (await readAirSwapOwner(provider,target,at)!==p.wallet) throw Error('У этого модуля другой владелец.');
}

export function addModule(rows,p,mod) {
  const target=mod.toLowerCase();
  if (!address.test(target) || /^0x0+$/.test(target) || !rows.some(line=>matches(p,fields(line)))) {
    throw Error('Настройки контракта изменились. Откройте размещение заново.');
  }
  const line=[String(p.chainId),p.kind.slice(-2),p.exchange,p.exchangeHash,target,p.runtimeKeccak].join(' ');
  return rows.some(row=>fields(row).join(' ')===line) ? rows : [...rows,line];
}
