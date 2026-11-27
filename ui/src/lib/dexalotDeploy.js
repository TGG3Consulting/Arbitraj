import {DEXALOT_CHAIN_ID} from './dexalotNetwork.js';
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';
import {findWalletModule} from './registeredWalletModule.js';

const addr=/^0x[0-9a-f]{40}$/, digest=/^0x[0-9a-f]{64}$/;
const slot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
const fields=row=>typeof row==='string'?row.trim().toLowerCase().split(/\s+/):[];
const word=a=>a.slice(2).padStart(64,'0');
const input=name=>'0x'+sha3.keccak_256(name).slice(0,8);
function roleMatches(item,row){
  if(!item || ![3,5].includes(row.length) || row[1]!==item.address || row[2]!==item.hash) return false;
  return item.implementation ? row.length===5 && row[3]===item.implementation.address && row[4]===item.implementation.hash : row.length===3;
}

export async function loadDexalot(s,p,sha) {
  const rows=(s.rows??[]).map(fields),wallet=s.wallet?.toLowerCase();
  if(s.kind!=='dexalot_rfq' || p.kind!==s.kind || p.chainId!==DEXALOT_CHAIN_ID || Number(s.chainId)!==DEXALOT_CHAIN_ID ||
     !addr.test(wallet) || /^0x0+$/.test(wallet) || !addr.test(p.router) || !addr.test(p.wrapped) ||
     !digest.test(p.sourceBlock) || !digest.test(p.runtimeKeccak) || !/^[0-9a-f]{64}$/.test(p.runtimeSha256) ||
     !p.data.endsWith(word(p.router)+word(p.wrapped)) || await sha(p.data)!==p.creationSha256 ||
     !rows.some(row=>row[0]==='router' && roleMatches(p.roles[p.router],row))) {
    throw Error('Проверьте кошелёк, сеть Base и исходный контракт маршрутизации Dexalot.');
  }
  const makers=rows.filter(row=>row[0]==='maker' && row[1]!==p.router && roleMatches(p.roles[row[1]],row)).map(row=>row[1]);
  if(!makers.length) throw Error('Нужен хотя бы один проверенный контракт расчётов Dexalot.');
  return {...p,wallet,makers:[...new Set(makers)]};
}

export async function checkDexalot(provider,p,at) {
  if(at?.requireCanonical!==true || !digest.test(at.blockHash)) throw Error('Нужен канонический блок Base.');
  const rpc=(method,params)=>provider.request({method,params});
  async function verify(r){
    const code=await rpc('eth_getCode',[r.address,at]);
    if(code==='0x' || runtimeKeccak(code)!==r.hash) return false;
    if(!r.implementation) return true;
    const state=await rpc('eth_getStorageAt',[r.address,slot,at]);
    if(state.toLowerCase()!=='0x'+word(r.implementation.address)) return false;
    const impl=await rpc('eth_getCode',[r.implementation.address,at]);
    return impl!=='0x' && runtimeKeccak(impl)===r.implementation.hash;
  }
  if(!await verify(p.roles[p.router])) throw Error('Изменился исходный роутер или его реализация.');
  const data=await rpc('eth_call',[{to:p.router,data:input('getAllowedRFQs()')},at]);
  if(!/^0x(?:[a-f0-9]{64}){2,}$/i.test(data)) throw Error('В реестре получен некорректный ответ.');
  const words=data.slice(2).toLowerCase().match(/.{64}/g);
  if(BigInt('0x'+words[0])!==32n || BigInt('0x'+words[1])!==BigInt(words.length-2)) throw Error('Некорректная длина реестра Dexalot.');
  const available=new Set(words.slice(2));
  const results=await Promise.allSettled(p.makers.filter(a=>available.has(word(a))).map(a=>verify(p.roles[a])));
  if(!results.some(r=>r.status==='fulfilled' && r.value)) throw Error('Для выбранных касс не удалось подтвердить запись реестра и код на одном блоке.');
}

export function registeredDexalot(provider,p,rows){
  return findWalletModule(provider,p,rows.map(fields).filter(r=>r.length===3 && r[0]==='module' && addr.test(r[1]) && r[2]===p.runtimeKeccak).map(r=>r[1]));
}

export function addDexalot(rows,p,target) {
  const a=target.toLowerCase();
  if(!addr.test(a) || /^0x0+$/.test(a) || !rows.some(row=>{const f=fields(row);return f[0]==='router' && roleMatches(p.roles[p.router],f);})){
    throw Error('Изменилась конфигурация Dexalot.');
  }
  const line=['module',a,p.runtimeKeccak].join(' ');
  return rows.some(v=>fields(v).join(' ')===line)?rows:[...rows,line];
}
