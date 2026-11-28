// Проверяет каждую книгу официального Base manifest и связывает её с отдельным результатом
// eth_call CREATE. Вызывает verify-raindex-base-card.mjs при записи и автономной сверке CI.
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import sha3 from 'js-sha3';

const selectors={3:'8a44689c',4:'0997c4a0',5:'d0f7a862',6:'69c72856',7:'b0460f07'};
const zero='0x'+'0'.repeat(64);
const keccak=bytes=>'0x'+sha3.keccak256(bytes);
const sha256=bytes=>createHash('sha256').update(bytes).digest('hex');

/** Привязывает три immutable к результату изолированного выполнения конструктора. */
export function constructorRuntime(template, groups, words, expectedKeccak) {
  const matches=[];
  for(const order of [[0,1,2],[0,2,1],[1,0,2],[1,2,0],[2,0,1],[2,1,0]]){
    const runtime=Buffer.from(template);
    for(const [i,group] of groups.entries())
      for(const {start} of group) words[order[i]].copy(runtime,start);
    if(keccak(runtime)===expectedKeccak) matches.push(runtime);
  }
  assert.equal(matches.length,1,'compiled immutables do not match the isolated constructor');
  return matches[0];
}

/** У опубликованных книг с отдельной кассой сохраняет измерение; все остальные получают
 * только подтверждение кода и конструктора. Остаток USDC не ограничивает список книг. */
export function bindManifest(card,compiled,template,groups,inventory,creates){
  assert.equal(creates.chainId,card.chainId);
  assert.equal(inventory.chain_id,card.chainId);
  assert.equal(creates.height,inventory.height);
  assert.deepEqual(creates.selector,inventory.selector);
  assert.equal(creates.selector.blockHash,card.sourceBlock);
  assert.equal(creates.selector.requireCanonical,true);
  assert.equal(creates.manifestSha256,inventory.source_sha256);
  const market=new Map(inventory.books.map(row=>[row.book,row]));
  const known=new Map(card.books.map(row=>[row.book,row]));
  const recorded=new Set();
  assert.equal(market.size,inventory.books.length);
  assert.equal(creates.entries.length,market.size);
  for(const r of creates.entries){
    assert(!recorded.has(r.book));recorded.add(r.book);
    const m=market.get(r.book);
    assert(m && m.version===r.version && m.start_block===r.startBlock);
    assert(r.startBlock>0 && r.startBlock<=creates.height);
    assert.match(r.birth,/^0x[0-9a-f]{64}$/);
    assert.match(r.beforeBirth,/^0x[0-9a-f]{64}$/);
    assert.notEqual(r.birth,r.beforeBirth);
    assert.equal(r.beforeCode,'0x');
    assert.equal(r.birthCodeHash,m.code_hash);
    assert.equal(r.implementation,zero);
    assert.equal(r.selector,'0x'+selectors[r.version],`missing takeOrders ABI V${r.version}, ${r.book}`);
    assert.equal(r.selector,'0x'+m.take_selector);
    assert(m.push4_take);
    const code=Buffer.from(creates.codes[r.bookHash].slice(2),'hex');
    assert.equal(keccak(code),m.code_hash);
    assert.equal(r.bookHash,m.code_hash);
    assert.equal(code.length,m.code_bytes);
    const words=[r.book.slice(2).padStart(64,'0'),r.bookHash.slice(2),r.selector.slice(2).padEnd(64,'0')].map(s=>Buffer.from(s,'hex'));
    const init=Buffer.concat([Buffer.from(compiled.creation,'hex'),...words]);
    assert.equal(sha256(init),r.initSha256);
    const bytes=Buffer.from(r.runtime.slice(2),'hex');
    const runtimeKeccak=keccak(bytes);
    assert(bytes.equals(constructorRuntime(template,groups,words,runtimeKeccak)));
    const common={version:r.version,book:r.book,bookHash:r.bookHash,takeSelector:r.selector,
      runtimeKeccak,runtimeSha256:sha256(bytes),creationSha256:r.initSha256};
    if(known.has(r.book)){
      const old=known.get(r.book);
      for(const [key,value] of Object.entries(common)) assert.equal(old[key],value);
      old.startBlock=r.startBlock;
    } else {
      card.books.push({...common,sourceKind:'rpc',url:String(r.startBlock),settlementVerified:null,
        projectFundingVerified:null,startBlock:r.startBlock});
    }
  }
  for(const address of known.keys()) assert(recorded.has(address));
}
