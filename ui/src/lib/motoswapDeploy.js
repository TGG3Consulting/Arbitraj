import {runtimeKeccak} from './uniswapV1Deploy.js';

const names=['factory','fee_router','core','beacon','pair_impl','registry','creator','vault','collector'];
const proxies=new Set(['factory','fee_router','collector']);
const address=/^0x[0-9a-f]{40}$/;
const hash=/^0x[0-9a-f]{64}$/;
const slot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
const zero='0x'+'0'.repeat(64);
const word=a=>'0x'+a.slice(2).padStart(64,'0');
function requireValue(ok,message){if(!ok)throw Error(message);}

function bindings(rows){
  requireValue(Array.isArray(rows) && rows.length===names.length,'Укажите девять подтверждённых ролей Motoswap.');
  const roles={};
  for(const row of rows){
    const parts=String(row).trim().toLowerCase().split(/\s+/);
    const [name,target,code,implementation,implCode]=parts;
    requireValue(names.includes(name) && !roles[name] && parts.length===(proxies.has(name)?5:3)
      && address.test(target) && word(target)!==zero && hash.test(code) && code!==zero &&
      (!implementation || (address.test(implementation) && word(implementation)!==zero && hash.test(implCode) && implCode!==zero)),
      'Проверьте адреса и хеши ролей Motoswap.');
    roles[name]={target,code,implementation,implCode};
  }
  return roles;
}

export async function loadMoto(settings,plan,digest){
  requireValue(settings.kind==='motoswap_v2' && plan.kind===settings.kind && plan.chainId===1 && String(settings.chainId)==='1'
    && hash.test(plan.sourceBlock) && hash.test(plan.runtimeKeccak) && /^[0-9a-f]{64}$/.test(plan.runtimeSha256),
    'Для Motoswap нужен подтверждённый архив Ethereum.');
  const roles=bindings(settings.motoRoles);
  const source=bindings(plan.roleRows);
  for(const name of names)
    requireValue(JSON.stringify(roles[name])===JSON.stringify(source[name]),'Архив и настройки роли Motoswap расходятся.');
  requireValue(String(settings.market).toLowerCase()===roles.factory.target && String(settings.quoter).toLowerCase()===roles.fee_router.target &&
    settings.hashes?.some(h=>String(h).toLowerCase()===plan.runtimeKeccak),'Выберите подтверждённую фабрику, FeeRouter и runtime MotoswapWallet.');
  requireValue(/^0x(?:[0-9a-f]{2})+$/.test(plan.data) && plan.data.endsWith(word(roles.factory.target).slice(2)+word(roles.fee_router.target).slice(2)) &&
    await digest(plan.data)===plan.creationSha256,'Изменились код CREATE или аргументы MotoswapWallet.');
  return {...plan,roles};
}

const rpc=(provider,method,params)=>provider.request({method,params});
async function linked(provider,at,target,selector,wanted){
  const result=await rpc(provider,'eth_call',[{to:target,data:selector},at]);
  requireValue(result?.toLowerCase()===word(wanted),'Действующий контракт Motoswap вернул другой адрес.');
}

export async function checkMoto(provider,plan,at){
  requireValue(at?.requireCanonical===true && hash.test(at.blockHash),'Нужен канонический блок Ethereum.');
  const r=plan.roles;
  await Promise.all(Object.values(r).map(async ({target,code,implementation,implCode})=>{
    const [body,stored]=await Promise.all([rpc(provider,'eth_getCode',[target,at]),rpc(provider,'eth_getStorageAt',[target,slot,at])]);
    requireValue(body!=='0x' && runtimeKeccak(body)===code && stored?.toLowerCase()===(implementation?word(implementation):zero),
      'Изменились код или реализация контракта Motoswap.');
    if(implementation){
      const value=await rpc(provider,'eth_getCode',[implementation,at]);
      requireValue(value!=='0x' && runtimeKeccak(value)===implCode,'Изменился код реализации Motoswap.');
    }
  }));
  const links=[['fee_router','0xf887ea40','core'],['core','0xc45a0155','factory'],['factory','0x59659e90','beacon'],
    ['factory','0x5c60da1b','pair_impl'],['beacon','0x5c60da1b','pair_impl'],['fee_router','0x7b103999','registry'],
    ['fee_router','0x9a6a5cf0','creator'],['fee_router','0x29269144','vault'],['fee_router','0x913e77ad','collector']];
  await Promise.all(links.map(([from,sig,to])=>linked(provider,at,r[from].target,sig,r[to].target)));
}

export async function checkMotoModule(provider,plan,contract,at){
  await Promise.all([linked(provider,at,contract,'0xc45a0155',plan.roles.factory.target),
    linked(provider,at,contract,'0xf887ea40',plan.roles.fee_router.target)]);
}

