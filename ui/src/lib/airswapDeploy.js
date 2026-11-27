import {findWalletModule} from './registeredWalletModule.js';
export {checkWalletOwner as checkAirSwapOwner} from './registeredWalletModule.js';
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
  return findWalletModule(provider,p,candidates.map(row=>row[4]));
}

export function addModule(rows,p,mod) {
  const target=mod.toLowerCase();
  if (!address.test(target) || /^0x0+$/.test(target) || !rows.some(line=>matches(p,fields(line)))) {
    throw Error('Настройки контракта изменились. Откройте размещение заново.');
  }
  const line=[String(p.chainId),p.kind.slice(-2),p.exchange,p.exchangeHash,target,p.runtimeKeccak].join(' ');
  return rows.some(row=>fields(row).join(' ')===line) ? rows : [...rows,line];
}
