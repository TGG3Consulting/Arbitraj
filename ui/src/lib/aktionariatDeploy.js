import {runtimeKeccak} from './uniswapV1Deploy.js';

const address = /^0x[0-9a-f]{40}$/;
const digest = /^0x[0-9a-f]{64}$/;
const same = (a,b) => typeof a === 'string' && a.toLowerCase() === b;
const word = value => value.replace(/^0x/,'').padStart(64,'0');
const validAddress = a => address.test(a) && !/^0x0+$/.test(a);

// Выбор по сохранённому отпечатку сохраняет повтор CREATE для уже настроенной площадки.
export function usesOriginalAktionariat(hashes) {
  return hashes?.length === 1 && same(hashes[0], '0xc89be9fbbce19d4f938d94776d4eec71574a276bc61bf8bbe8ff158a670857d3');
}

export async function loadAktionariat(settings,plan,sha) {
  if (settings.kind !== 'aktionariat' || plan.kind !== settings.kind || plan.chainId !== 1 ||
      String(settings.chainId) !== '1' || !digest.test(plan.runtimeKeccak) || !digest.test(plan.sourceBlock) ||
      !settings.hashes?.some(h=>same(h,plan.runtimeKeccak))) throw Error('Профиль Aktionariat расходится с архивом Ethereum.');
  for (const [role,configured,fingerprint] of [['registry','market','factoryHash'],['hub','quoter','quoterHash']]) {
    if (!validAddress(plan[role]) || !digest.test(plan[role+'Hash']) ||
        !same(settings[configured],plan[role]) || !same(settings[fingerprint],plan[role+'Hash']))
      throw Error('Изменились адрес или код контракта Aktionariat.');
  }
  if (!Array.isArray(plan.issuerMarkets) || !Array.isArray(plan.issuerRoles) ||
      plan.issuerMarkets.length !== plan.issuerRoles.length || new Set(plan.issuerMarkets).size !== plan.issuerMarkets.length ||
      !plan.issuerRoles.every((r,i)=>r.address===plan.issuerMarkets[i] && [r.address,r.cash,r.share].every(validAddress) &&
          r.cash!==r.share && validAddress(r.owner) && digest.test(r.runtime))) throw Error('Список дополнительных брокеров отличается от архива.');
  const configured = (settings.brokerMarkets ?? []).map(line=>line.toLowerCase().trim().split(/\s+/));
  for (const r of plan.issuerRoles) {
    const rows=configured.filter(fields=>fields[0]===r.address);
    const expected=[r.address,r.cash,r.share,plan.hub,r.runtime,plan.hubHash,r.owner];
    if (rows.length!==1 || expected.some((field,i)=>rows[0][i]!==field))
      throw Error('В настройках указаны другие условия эмитента '+r.address+'.');
  }
  const args = [plan.registry,plan.hub,'60',plan.issuerMarkets.length.toString(16),...plan.issuerMarkets].map(word).join('');
  if (!plan.data.endsWith(args) || await sha(plan.data)!==plan.creationSha256) throw Error('Изменился код создания или список эмитентов.');
  return plan;
}

export async function checkAktionariat(provider,p,at) {
  if (at?.requireCanonical !== true || !digest.test(at.blockHash)) throw Error('Для проверки нужен канонический блок Ethereum.');
  const get=(method,params)=>provider.request({method,params});
  const roles=[[p.registry,p.registryHash],[p.hub,p.hubHash],...p.issuerRoles.map(r=>[r.address,r.runtime])];
  await Promise.all(roles.map(async ([addr,hash])=>{
    if (runtimeKeccak(await get('eth_getCode',[addr,at]))!==hash) throw Error('Изменился код контракта '+addr+'.');
  }));
  await Promise.all(p.issuerRoles.flatMap(r=>[['0x777adcf0',p.hub],['0x5001f3b5',r.cash],['0xfc0c546a',r.share]].map(async ([selector,addr])=>{
    if (!same(await get('eth_call',[{to:r.address,data:selector},at]),'0x'+word(addr)))
      throw Error('Брокер изменил токен акции, валюту или платёжный контракт.');
  })));
  await Promise.all(p.issuerRoles.flatMap(r=>[r.address,r.share].map(async to=>{
    if (!same(await get('eth_call',[{to,data:'0x8da5cb5b'},at]),'0x'+word(r.owner)))
      throw Error('У брокера или токена акции сменился эмитент.');
  })));
}
