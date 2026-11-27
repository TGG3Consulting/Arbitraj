// CREATE проверенного кошелька Algebra, закреплённый в архиве Ethereum.
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address = /^0x[0-9a-f]{40}$/;
const hash = /^0x[0-9a-f]{64}$/;
const same = (left,right) => typeof left === 'string' && left.toLowerCase() === right;
const word = value => value.slice(2).padStart(64,'0');

export async function loadAlgebra(settings,plan,sha) {
  if (settings.kind !== 'algebra_integral_theorem' || plan.kind !== settings.kind ||
      plan.chainId !== 1 || String(settings.chainId) !== '1' || !hash.test(plan.sourceBlock) ||
      !hash.test(plan.runtimeKeccak) || !settings.hashes?.some(h=>same(h,plan.runtimeKeccak))) {
    throw Error('Профиль Theorem и архив кошелька расходятся.');
  }
  for (const [key, role] of [['market','factory'],['officialRouter','router'],['deployer','deployer']]) {
    if (!address.test(plan[role]) || /^0x0+$/.test(plan[role]) || !same(settings[key],plan[role]) ||
        !hash.test(plan[role+'Hash'])) throw Error('Не подтверждена роль '+role+'.');
  }
  if (!same(settings.factoryHash,plan.factoryHash) || !same(settings.routerHash,plan.routerHash) ||
      !plan.data.endsWith(word(plan.factory)+word(plan.router)) || await sha(plan.data)!==plan.creationSha256)
    throw Error('Изменились контракты либо код создания кошелька Algebra.');
  return plan;
}

export async function checkAlgebra(provider,p,at) {
  if (at?.requireCanonical!==true || !hash.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  const get = (method,params) => provider.request({method,params});
  await Promise.all(['factory','router','deployer'].map(async role=>{
    if(runtimeKeccak(await get('eth_getCode',[p[role],at]))!==p[role+'Hash']) throw Error('Изменился код '+role+'.');
  }));
  for (const [to,selector,expected] of [[p.router,'0xc45a0155',p.factory],
      [p.router,'0x3119049a',p.deployer],[p.factory,'0x3119049a',p.deployer]]) {
    if (!same(await get('eth_call',[{to,data:selector},at]),'0x'+word(expected)))
      throw Error('Регистрация Router или адрес деплоера Algebra изменились.');
  }
}
