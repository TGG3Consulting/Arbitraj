// Карточка RaindexWallet: CREATE отдельной книги, три immutable-роли и исходные строки raindex.books.
// Используют кошелёк и тесты по raindex_base_protected_wallets.json.gz. ABI: contracts/RaindexWallet.sol.
import sha3 from 'js-sha3';
import {runtimeKeccak} from './uniswapV1Deploy.js';

const address=/^0x(?!0{40}$)[0-9a-f]{40}$/i;
const hash=/^0x(?!0{64}$)[0-9a-f]{64}$/i;
const sha=/^[0-9a-f]{64}$/;
const selectors={3:'0x8a44689c',4:'0x0997c4a0',5:'0xd0f7a862',6:'0x69c72856',7:'0xb0460f07'};
const implementationSlot='0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc';
const zeroWord='0x'+'0'.repeat(64);
const addrWord=a=>a.slice(2).toLowerCase().padStart(64,'0');

/** Допустимый источник ордеров, как в raindex/profile.rs. */
function source(version,kind,url) {
  // При чтении событий выбранного узла последняя колонка содержит блок размещения книги.
  if(kind==='rpc') return Number(version)===5 && typeof url==='string' && /^0*[1-9][0-9]{0,19}$/.test(url) && BigInt(url)<=0xffffffffffffffffn;
  if(!['graphql','blockscout'].includes(kind) || !URL.canParse(url)) return false;
  const u=new URL(url);
  return u.protocol==='https:' && !u.username && !u.password && !/[?#]/.test(url);
}

/** Любая повреждённая/дублированная строка делает профиль непригодным и в торговом адаптере. */
function readRows(rows,plan) {
  if(rows!==null && !Array.isArray(rows)) throw Error('Проверьте список raindex.books.');
  const books=new Map();
  for(const row of rows??[]) {
    const [chain,version,book,bookHash,module,moduleHash,kind,url,...tail]=String(row).trim().split(/\s+/);
    if(tail.length || !/^\+?\d+$/.test(chain) || Number(chain)!==plan.chainId ||
        !/^\+?\d+$/.test(version) || !Object.hasOwn(selectors,Number(version)) ||
        !address.test(book) || !hash.test(bookHash) || !address.test(module) || !hash.test(moduleHash) ||
        book.toLowerCase()===module.toLowerCase() || books.has(book.toLowerCase()) || !source(version,kind,url)) {
      throw Error('Проверьте сеть, версии, адреса и источники в raindex.books.');
    }
    books.set(book.toLowerCase(),{version:Number(version),bookHash:bookHash.toLowerCase(),module:module.toLowerCase(),moduleHash:moduleHash.toLowerCase()});
  }
  const row=books.get(plan.book);
  if(row && (row.version!==plan.version || row.bookHash!==plan.bookHash || row.moduleHash!==plan.runtimeKeccak)) {
    throw Error('Существующая строка этой книги отличается от архивного профиля Raindex.');
  }
  return row;
}

/** Возвращает CREATE с ABI (address,bytes32,bytes4) для одной архивной книги. */
export async function loadRaindex(settings,card,digest) {
  const book=card.books?.find(x=>x.book===settings.book);
  if(settings.kind!=='raindex' || card.kind!==settings.kind || card.chainId!==8453 || settings.chainId!==card.chainId ||
      !address.test(settings.wallet) || !hash.test(card.sourceBlock) || !book || !address.test(book.book) ||
      !hash.test(book.bookHash) || !hash.test(book.runtimeKeccak) || !sha.test(book.runtimeSha256) ||
      selectors[book.version]!==book.takeSelector || !book.takeSelector || !source(book.version,book.sourceKind,book.url)) {
    throw Error('Проверьте кошелёк, сеть Base и архивное размещение книги Raindex.');
  }
  const data=card.creation+addrWord(book.book)+book.bookHash.slice(2)+book.takeSelector.slice(2).padEnd(64,'0');
  if(await digest(data)!==book.creationSha256) throw Error('Байткод или аргументы конструктора RaindexWallet изменились.');
  const plan={...book,kind:card.kind,chainId:card.chainId,sourceBlock:card.sourceBlock,data,wallet:settings.wallet.toLowerCase()};
  readRows(settings.rows,plan);
  return plan;
}

/** Сверяет runtime и отсутствие EIP-1967 implementation в том же состоянии, что проверяет адаптер. */
async function plain(provider,target,expected,at,knownCode) {
  if(at?.requireCanonical!==true || !hash.test(at.blockHash)) throw Error('Для проверки Raindex нужен канонический блок.');
  const [code,slot]=await Promise.all([
    knownCode ?? provider.request({method:'eth_getCode',params:[target,at]}),
    provider.request({method:'eth_getStorageAt',params:[target,implementationSlot,at]}),
  ]);
  if(runtimeKeccak(code)!==expected || slot?.toLowerCase()!==zeroWord) throw Error('Изменился код или реализация '+target+'.');
}

/** Подтверждает код выбранной книги до оценки и подписи. */
export const checkRaindexBook=(provider,plan,at)=>plain(provider,plan.book,plan.bookHash,at);

/** У модуля нет owner(). Денежный вызов принадлежит msg.sender, три публичных getter привязывают модуль к книге. */
export async function checkRaindexModule(provider,p,target,at,knownCode) {
  await plain(provider,target,p.runtimeKeccak,at,knownCode);
  const roles=[['book()','0x'+addrWord(p.book)],['bookCodeHash()',p.bookHash],['takeSelector()',p.takeSelector.padEnd(66,'0')]];
  await Promise.all(roles.map(async ([name,expected])=>{
    const data='0x'+sha3.keccak_256(name).slice(0,8);
    const got=await provider.request({method:'eth_call',params:[{to:target,data},at]});
    if(got?.toLowerCase()!==expected) throw Error('RaindexWallet: расходится '+name+'.');
  }));
}

/** Проверяет уже сохранённый общий адрес книги перед новым CREATE. */
export async function registeredRaindex(provider,p,rows) {
  const row=readRows(rows,p);
  if(!row) return null;
  if(BigInt(await provider.request({method:'eth_chainId',params:[]}))!==BigInt(p.chainId)) throw Error('Выберите Base в кошельке.');
  const head=await provider.request({method:'eth_getBlockByNumber',params:['finalized',false]});
  const at={blockHash:head?.hash,requireCanonical:true};
  await Promise.all([checkRaindexBook(provider,p,at),checkRaindexModule(provider,p,row.module,at)]);
  return row.module;
}

/** Дополняет список одной оплаченной книгой. При повторе сохраняет исходный текст и URL оператора. */
export function addRaindex(rows,p,target) {
  const row=readRows(rows,p);
  if(!address.test(target) || target.toLowerCase()===p.book) throw Error('Некорректный адрес модуля книги.');
  if(row && row.module!==target.toLowerCase()) throw Error('В raindex.books у этой книги уже задан другой модуль.');
  if(row) return rows;
  return [...(rows??[]),[p.chainId,p.version,p.book,p.bookHash,target.toLowerCase(),p.runtimeKeccak,p.sourceKind,p.url].join(' ')];
}
