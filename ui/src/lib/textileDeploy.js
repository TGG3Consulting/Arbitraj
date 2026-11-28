// CREATE кошелька Textile Base. Байты трех immutable получены по двум архивным вызовам конструктора.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';
import {findWalletModule} from './registeredWalletModule.js';
const address=/^0x[0-9a-f]{40}$/, hash=/^0x[0-9a-f]{64}$/;
const word=a=>a.slice(2).padStart(64,'0');
const fields=s=>typeof s==='string'?s.trim().toLowerCase().split(/\s+/):[];
const validAddress=a=>address.test(a) && !/^0x0+$/.test(a);
const labels={reactor:'reactor',fee_controller:'fee',permit2:'permit2'};

export async function loadTextile(settings,p,sha) {
  const wallet=settings.wallet?.toLowerCase(), rows=(settings.rows??[]).map(fields);
  if(p.kind!=='textile_wallet' || settings.kind!==p.kind || p.chainId!==8453 ||
     Number(settings.chainId)!==p.chainId || !validAddress(wallet) || !hash.test(p.sourceBlock) ||
     await sha(p.creation)!==p.creationTemplateSha256 || await sha(p.runtime)!==p.runtimeTemplateSha256)
    throw Error('Проверьте карточку Textile, кошелёк и сеть Base.');
  for(const [key,label] of Object.entries(labels)) {
    const role=p.roles[key], matches=rows.filter(r=>r[0]===String(p.chainId) && r[1]===label);
    if(!role || matches.length!==1 || matches[0].length!==4 ||
       matches[0][2]!==role.address || matches[0][3]!==role.runtime)
      throw Error('Проверьте роль '+label+' в textile.contracts.');
  }
  const roles={owner:wallet,reactor:p.roles.reactor.address,reactorHash:p.roles.reactor.runtime};
  const data=p.creation+Object.values(roles).map(word).join('');
  const bytes=p.runtime.slice(2).match(/../g);
  for(const [name,value] of Object.entries(roles)){
    const refs=p.offsets[name];
    if(!Array.isArray(refs) || !refs.length) throw Error('Нет привязки '+name+' в карточке Textile.');
    for(const offset of refs){
      if(!Number.isInteger(offset) || offset<0 || bytes.slice(offset,offset+32).join('')!=='00'.repeat(32))
        throw Error('Неверное положение '+name+' в карточке Textile.');
      bytes.splice(offset,32,...word(value).match(/../g));
    }
  }
  const runtime='0x'+bytes.join('');
  return {...p,wallet,data,creationSha256:await sha(data),runtimeSha256:await sha(runtime),runtimeKeccak:runtimeKeccak(runtime)};
}

export async function checkTextile(provider,p,at){
  if(at?.requireCanonical!==true || !hash.test(at.blockHash)) throw Error('Требуется канонический блок Base.');
  const rpc=(method,params)=>provider.request({method,params});
  await Promise.all(Object.values(p.roles).map(async role=>{
    const code=await rpc('eth_getCode',[role.address,at]);
    if(!code || code==='0x' || runtimeKeccak(code)!==role.runtime) throw Error('Код роли Textile изменился.');
  }));
  for(const [getter,r] of [['permit2()',p.roles.permit2],['feeController()',p.roles.fee_controller]]){
    const data='0x'+sha3.keccak_256(getter).slice(0,8);
    const result=await rpc('eth_call',[{to:p.roles.reactor.address,data},at]);
    if(result.toLowerCase()!=='0x'+word(r.address)) throw Error('Связь Reactor с '+getter+' изменилась.');
  }
}

const walletRows=(rows,p)=>rows.map(fields).filter(x=>x[0]===String(p.chainId) && x[1]==='wallet');
export function registeredTextile(provider,p,rows){
  const own=walletRows(rows,p).filter(x=>x.length===4 || (x.length===5 && x[4]===p.wallet));
  return findWalletModule(provider,p,own.filter(x=>validAddress(x[2]) && x[3]===p.runtimeKeccak).map(x=>x[2]));
}
export function addTextile(rows,p,address){
  const module=address.toLowerCase();
  if(!validAddress(module))throw Error('Нет адреса модуля Textile.');
  const same=walletRows(rows,p).filter(x=>x.length===5 && x[4]===p.wallet);
  if(same.length){
    if(same.length===1 && same[0][2]===module && same[0][3]===p.runtimeKeccak) return rows;
    throw Error('Для этого владельца уже зарегистрирован другой модуль Textile.');
  }
  return [...rows,[p.chainId,'wallet',module,p.runtimeKeccak,p.wallet].join(' ')];
}
