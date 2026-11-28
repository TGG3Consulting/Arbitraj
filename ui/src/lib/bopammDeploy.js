// Роли и три immutable аргумента BopAmmWallet на Ethereum.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address=/^0x(?!0{40}$)[0-9a-f]{40}$/i;
const hash=/^0x(?!0{64}$)[0-9a-f]{64}$/i;
const word=a=>a.slice(2).padStart(64,'0').toLowerCase();
const jsonCall=(provider,method,params)=>provider.request({method,params});
const moduleKeys=['module','cash_module'];
const moduleKey=plan=>plan.moduleRole??'module';
const roleNames=['market','pricing','registry'];

function components(rows,plan) {
  if(!Array.isArray(rows)) throw Error('Укажите три рыночные роли в bopamm.roles.');
  const roles={},addresses=new Set();
  for(const row of rows) {
    const [name,target,code,...tail]=String(row).trim().split(/\s+/);
    if(![...roleNames,...moduleKeys].includes(name) || Object.hasOwn(roles,name) || tail.length ||
        !address.test(target) || !hash.test(code) || addresses.has(target.toLowerCase())) throw Error('Проверьте поля и уникальность bopamm.roles.');
    const id=target.toLowerCase(), digest=code.toLowerCase();
    if(moduleKeys.includes(name) ? (name===moduleKey(plan) && digest!==plan.runtimeKeccak) :
      (id!==plan.roles[name]?.address || digest!==plan.roles[name]?.hash)) throw Error('Привязка роли bopAMM расходится с архивом.');
    roles[name]=id;addresses.add(id);
  }
  if(!roleNames.every(k=>roles[k])) throw Error('В bopamm.roles нужны market, pricing, registry.');
  return roles;
}

export async function loadBop(settings,plan,sha) {
  if(settings.kind!=='bopamm' || plan.kind!==settings.kind || settings.chainId!==1 || plan.chainId!==1 ||
      !address.test(settings.wallet) || !address.test(plan.usdc) || !hash.test(plan.sourceBlock) ||
      !hash.test(plan.runtimeKeccak) || !/^[0-9a-f]{64}$/.test(plan.runtimeSha256) || !plan.roles) throw Error('Проверьте Ethereum, адрес плательщика и архив bopAMM.');
  const binds=components(settings.rows,plan);
  if(moduleKey(plan)==='cash_module') {
    const rows=settings.marketBindings?.cashRows;
    const expected=Object.entries(plan.cash.roles).map(([name,r])=>[name,r.address,r.hash].join(' '));
    expected.push('fee '+plan.cash.fee);
    const canon=row=>typeof row==='string' ? row.trim().toLowerCase().replace(/\s+/g,' ') : '';
    const actual=new Set(Array.isArray(rows)?rows.map(canon):[]);
    if(actual.size!==expected.length || rows.length!==expected.length || expected.some(r=>!actual.has(r)))
      throw Error('Проверьте семь ролей и комиссию в bopamm.cash_path для этой площадки Ethereum.');
  }
  const data=plan.creation+[binds.market,binds.pricing,plan.usdc].map(word).join('');
  if(await sha(data)!==plan.creationSha256) throw Error('Байткод или аргументы создания BopAmmWallet изменились.');
  return {...plan,data,wallet:settings.wallet.toLowerCase()};
}

async function getter(provider,to,name,expected,at,args='') {
  const raw=await jsonCall(provider,'eth_call',[{to,data:'0x'+sha3.keccak_256((args?name:name+'()')).slice(0,8)+args},at]);
  if(raw?.toLowerCase()!=='0x'+word(expected)) throw Error('bopAMM: изменена привязка '+name+'.');
}

export async function checkBopMarket(provider,p,at) {
  if(!at?.requireCanonical || !hash.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  if(p.cash) {
    const {roles:c,fee}=p.cash,tier='0x'+fee.toString(16);
    await Promise.all([
      ...Object.entries(c).map(async ([key,r])=>{
        if(runtimeKeccak(await jsonCall(provider,'eth_getCode',[r.address,at]))!==r.hash)
          throw Error('bopAMM: изменился код '+key+'.');
      }),
      ...[c.pool,c.router,c.quoter].map(r=>getter(provider,r.address,'factory',c.factory.address,at)),
      getter(provider,c.pool.address,'fee',tier,at),
      getter(provider,c.pool.address,'token0',c.usdc.address,at),
      getter(provider,c.pool.address,'token1',c.usdt.address,at),
      getter(provider,c.factory.address,'getPool(address,address,uint24)',c.pool.address,at,
        [c.usdc.address,c.usdt.address,tier].map(word).join(''))
    ]);
  }
  await Promise.all([
    ...roleNames.map(async key=>{
      const {address,hash}=p.roles[key];
      if(runtimeKeccak(await jsonCall(provider,'eth_getCode',[address,at]))!==hash) throw Error('bopAMM: изменился код '+key+'.');
    }),
    getter(provider,p.roles.market.address,'pricing',p.roles.pricing.address,at),
    getter(provider,p.roles.market.address,'usdc',p.usdc,at),
  ]);
}

export async function checkBopModule(provider,p,module,at) {
  if(p.cash) {
    const c=p.cash.roles;
    await Promise.all(Object.entries({WETH:c.weth.address,USDT:c.usdt.address,USDC:c.usdc.address,
      V3_ROUTER:c.router.address,V3_QUOTER:c.quoter.address,POOL_FEE:'0x'+p.cash.fee.toString(16)})
      .map(([k,v])=>getter(provider,module,k,v,at)));
  }
  await Promise.all([getter(provider,module,'market',p.roles.market.address,at),
    getter(provider,module,'pricing',p.roles.pricing.address,at),getter(provider,module,'usdc',p.usdc,at)]);
}

export async function registeredBop(provider,p,rows) {
  const module=components(rows,p)[moduleKey(p)];
  if(!module) return null;
  if(BigInt(await jsonCall(provider,'eth_chainId',[]))!==1n) throw Error('Выберите Ethereum в кошельке.');
  const block=await jsonCall(provider,'eth_getBlockByNumber',['finalized',false]);
  const at={blockHash:block?.hash,requireCanonical:true};
  await checkBopMarket(provider,p,at);
  if(runtimeKeccak(await jsonCall(provider,'eth_getCode',[module,at]))!==p.runtimeKeccak) throw Error('Отличается код BopAmmWallet.');
  await checkBopModule(provider,p,module,at);
  return module;
}

export function addBop(rows,p,target) {
  const module=components(rows,p)[moduleKey(p)];
  if(!address.test(target) || (module && module!==target.toLowerCase())) throw Error('Уже задан другой модуль bopAMM.');
  if(module) return rows;
  const result=[...rows,moduleKey(p)+' '+target.toLowerCase()+' '+p.runtimeKeccak];
  components(result,p);
  return result;
}
