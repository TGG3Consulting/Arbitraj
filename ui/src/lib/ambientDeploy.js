import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const roles={dex:'dex',query:'query',impact:'impact',hot:'1',knockout:'3500'};
const hexAddress=/^0x[0-9a-f]{40}$/, hash=/^0x[0-9a-f]{64}$/;
const word=value=>value.replace(/^0x/,'').padStart(64,'0');
const select=text=>'0x'+sha3.keccak_256(text).slice(0,8);
const equals=(value,expected)=>typeof value==='string' && value.toLowerCase()===expected;

export async function loadAmbient(s,p,sha) {
  if (s.kind!=='ambient_ethereum' || p.kind!==s.kind || p.chainId!==1 || String(s.chainId)!=='1' ||
      !hash.test(p.sourceBlock) || !hash.test(p.runtimeKeccak) || !/^[a-f0-9]{64}$/.test(p.runtimeSha256) ||
      !s.hashes?.some(h=>equals(h,p.runtimeKeccak)) || !s.ambientRoles?.length)
    throw Error('Проверьте сеть и хеш разрешённого модуля Ambient.');
  const configured=new Map();
  for(const line of s.ambientRoles){
    const [role,address,code,...extra]=line.trim().toLowerCase().split(/\s+/);
    if(configured.has(role) || extra.length) throw Error('Роль Ambient повторена или задана неверно.');
    configured.set(role,[address,code]);
  }
  for(const [name,slot] of Object.entries(roles)){
    const r=p.roles?.[slot], item=configured.get(name);
    if(!r || !hexAddress.test(r.address) || !hash.test(r.hash) || item?.[0]!==r.address || item?.[1]!==r.hash)
      throw Error('Проверьте роль Ambient '+name+' и хеш её кода.');
  }
  if (!hexAddress.test(p.wrapped) || p.exchange!==p.roles.dex.address || p.query!==p.roles.query.address ||
      !equals(s.market,p.exchange) || !equals(s.ambientQuery,p.query) || !equals(s.quoter,p.roles.impact.address) ||
      !equals(s.wrapped,p.wrapped) || !p.data.endsWith([p.exchange,p.query,p.wrapped].map(word).join('')) ||
      await sha(p.data)!==p.creationSha256) throw Error('Параметры создания Ambient отличаются от проверенных.');
  return p;
}

export async function checkAmbient(provider,p,at){
  if(at?.requireCanonical!==true || !hash.test(at.blockHash)) throw Error('Нужен канонический блок Ethereum.');
  const rpc=(method,params)=>provider.request({method,params});
  await Promise.all(Object.values(roles).map(async key=>{
    const r=p.roles[key];
    const code=await rpc('eth_getCode',[r.address,at]);
    if(runtimeKeccak(code)!==r.hash) throw Error('Изменился код контракта Ambient '+r.address+'.');
  }));
  const getters=[
    [p.query,select('dex_()'),p.exchange],[p.roles.impact.address,select('dex_()'),p.exchange],
    [p.exchange,select('readSlot(uint256)')+word('2'),p.roles['1'].address],
    [p.exchange,select('readSlot(uint256)')+word((3501).toString(16)),p.roles['3500'].address]
  ];
  await Promise.all(getters.map(async ([to,data,expected])=>{
    if(!equals(await rpc('eth_call',[{to,data},at]),'0x'+word(expected)))
      throw Error('В каноническом блоке изменился контракт исполнения или чтения Ambient.');
  }));
}
