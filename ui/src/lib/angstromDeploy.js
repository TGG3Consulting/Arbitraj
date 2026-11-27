// Проверенные аргументы CREATE модуля AngstromWallet и роли трёх исходных размещений Ethereum.
import {runtimeKeccak} from './uniswapV1Deploy.js';

const same = (a,b) => typeof a === 'string' && a.toLowerCase() === b;
const roles = ['adapter','hook','manager'];
const address = /^0x[0-9a-f]{40}$/;
const hash = /^0x[0-9a-f]{64}$/;
export async function loadAngstrom(settings, plan, sha) {
  if (settings.kind !== 'angstrom_l1' || plan.kind !== settings.kind || plan.chainId !== 1 ||
      String(settings.chainId) !== '1' || !hash.test(plan.runtimeKeccak) ||
      !settings.hashes?.some(h => same(h,plan.runtimeKeccak))) throw Error('Сеть или допустимый код AngstromWallet изменились.');
  const args = [];
  for (const role of roles) {
    const record = plan.roles?.[role];
    if (!address.test(record?.address) || !hash.test(record?.runtime)) throw Error('Неверная роль Angstrom.');
    if (role === 'manager' ? !same(settings.poolManager,record.address) :
      !settings.launchRoles?.some(r => same(r,role+'@'+record.address+'@'+record.runtime))) {
      throw Error('Адрес '+role+' отличается от выбранного профиля Angstrom.');
    }
    args.push(record.address.slice(2).padStart(64,'0'));
  }
  if (!plan.data.endsWith(args.join('')) || await sha(plan.data) !== plan.creationSha256)
    throw Error('Код создания или аргументы AngstromWallet изменились.');
  return plan;
}

export async function checkAngstrom(provider, plan, at) {
  if (at?.requireCanonical !== true || !hash.test(at.blockHash)) throw Error('Требуется канонический блок Ethereum.');
  for (const role of roles) {
    const record=plan.roles[role];
    const code=await provider.request({method:'eth_getCode',params:[record.address,at]});
    if (runtimeKeccak(code)!==record.runtime) throw Error('Код '+role+' изменился.');
  }
}
