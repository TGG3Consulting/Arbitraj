import {runtimeKeccak} from './uniswapV1Deploy.js';

const address=/^0x[0-9a-f]{40}$/;
const hash=/^0x[0-9a-f]{64}$/;
const sha=/^[0-9a-f]{64}$/;
const zero = '0x'+'0'.repeat(64);
const slot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
const word = value => '0x'+value.slice(2).padStart(64,'0');
const required = ['router','venue','vault','fallback'];
const intermediary = ['pool','logic','singleton'];

function need(condition,message) {
  if (!condition) throw Error(message);
}

function parseRoles(rows) {
  need(Array.isArray(rows) && [4,7].includes(rows.length),'Проверьте список ролей PropAMM.');
  const names=rows.length===4?required:[...required,...intermediary];
  const roles={};
  const seen=new Set();
  for (const row of rows) {
    const [name,target,code,implementation,implCode,...tail]=String(row).trim().toLowerCase().split(/\s+/);
    need(names.includes(name) && !roles[name] && address.test(target) && word(target)!==zero && !seen.has(target)
      && hash.test(code) && code!==zero && !tail.length && (!implementation || (address.test(implementation) && word(implementation)!==zero
      && implementation!==target && hash.test(implCode) && implCode!==zero)), 'Адрес, код или имя роли PropAMM расходятся с профилем.');
    roles[name]={target,code,implementation,implCode};seen.add(target);
  }
  need(names.every(k=>!!roles[k]) && roles.router.implementation,'Для публичного PropAMMRouter укажите проверенную реализацию.');
  return roles;
}

export async function loadPropVenue(settings,source,digest) {
  need(settings.kind==='lambda_propamm' && source.kind===settings.kind && source.chainId===1 && String(settings.chainId)==='1'
    && hash.test(source.sourceBlock),'Требуется архив PropVenueAdapter для Ethereum.');
  const roles=parseRoles(settings.propRoles);
  if (source.roles) {
    const previous=source.roles;
    need(Object.keys(previous).length===Object.keys(roles).length && Object.entries(roles).every(([name,r])=>
      ['target','code','implementation','implCode'].every(k=>r[k]===previous[name]?.[k])),
      'Настройки ролей изменились после подготовки CREATE.');
  }
  const market=roles.venue.target;
  const record=source.markets?.[market];
  const addresses=['router','venue','vault'].map(k=>roles[k].target);
  const shells=['router','venue','vault'].map(k=>roles[k].code);
  need(settings.market?.toLowerCase()===market && settings.quoter?.toLowerCase()===roles.router.target
    && record?.addresses?.length===3 && addresses.every((v,i)=>v===record.addresses[i])
    && record?.shells?.length===3 && shells.every((v,i)=>v===record.shells[i])
    && hash.test(record.runtimeKeccak) && sha.test(record.runtimeSha256)
    && settings.hashes?.some(h=>h.toLowerCase()===record.runtimeKeccak),'Выберите подтверждённую кассу, площадку, роутер и runtime модуля.');
  const data=source.creation+[...addresses.map(word),...shells].map(x=>x.slice(2)).join('');
  need(/^0x(?:[0-9a-f]{2})+$/.test(source.creation) && await digest(data)===record.creationSha256 && (!source.data || source.data===data),
    'Код CREATE или один из шести аргументов PropVenueAdapter изменился.');
  return {...source,...record,market,roles,data};
}

const read=(provider,method,params)=>provider.request({method,params});
async function getter(provider,at,to,selector,expected) {
  const result=await read(provider,'eth_call',[{to,data:selector},at]);
  need(result?.toLowerCase()===word(expected),'Контракт PropAMM вернул иную роль или хеш '+to+'.');
}

export async function checkPropVenue(provider,p,at) {
  need(at?.requireCanonical===true && hash.test(at.blockHash),'Для PropAMM нужен канонический блок.');
  const roles=p.roles;
  await Promise.all(Object.values(roles).map(async ({target,code,implementation,implCode})=>{
    const [actual,stored]=await Promise.all([read(provider,'eth_getCode',[target,at]),read(provider,'eth_getStorageAt',[target,slot,at])]);
    need(actual!=='0x' && runtimeKeccak(actual)===code && stored.toLowerCase()===(implementation?word(implementation):zero),
      'Изменились код или реализация роли PropAMM '+target+'.');
    if (implementation) {
      const body=await read(provider,'eth_getCode',[implementation,at]);
      need(body!=='0x' && runtimeKeccak(body)===implCode,'Изменился код реализации PropAMM '+implementation+'.');
    }
  }));
  const links=[['router','0x2f61968b','fallback']];
  if (roles.pool) links.push(['venue','0xb58d3f1f','pool'],['venue','0x104e010b','logic'],['pool','0xc405e442','logic'],
    ['pool','0x521eb273','vault'],['logic','0x521eb273','vault'],['vault','0xa619486e','singleton']);
  await Promise.all(links.map(([from,sig,to])=>getter(provider,at,roles[from].target,sig,roles[to].target)));
}

export async function checkPropModule(provider,p,target,at) {
  const expected=[...p.addresses,...p.shells];
  const getters=['0xf887ea40','0x80f55605','0xfbfa77cf','0x7e2d8a92','0x31574546','0xad5ab79b'];
  await Promise.all(getters.map((selector,i)=>getter(provider,at,target,selector,expected[i])));
}


export function selectPropDeployment(settings, pending, ordinary, slot, protectedSlot) {
  const market=settings.market?.toLowerCase();
  const allowed=new Set((settings.hashes??[]).map(value=>value.toLowerCase()));
  for (const artifact of [protectedSlot,slot,ordinary]) {
    const plan=artifact?.markets?.[market];
    if (plan && allowed.has(plan.runtimeKeccak) && (!pending || plan.creationSha256===pending.creation)) {
      return artifact;
    }
  }
  throw Error('Для этой кассы и исходного CREATE нет принятого архива размещения.');
}
