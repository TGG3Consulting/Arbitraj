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
  bindSales(card, inventory);
}

/** Сопоставляет одинаковые заявки двух исполнителей. У продажи считаются собственные
 * списание токена и прирост USDC; данные о покупке за USDC берутся из другого опыта. */
function bindSales(card,inventory) {
  const {direct,protected:protectedRun}=inventory.sample_replay;
  assert.deepEqual(direct.selector,inventory.selector);
  assert.deepEqual(protectedRun.selector,inventory.selector);
  assert.equal(protectedRun.payer,direct.payer);
  assert.deepEqual(protectedRun.wlth_funding,direct.wlth_funding);
  const funding=protectedRun.wlth_funding;
  assert(BigInt(funding.before[0])>BigInt(funding.after[0]));
  assert(BigInt(funding.after[1])>BigInt(funding.before[1]));
  const identity=r=>JSON.stringify([r.book,r.version,r.order,r.i,r.j,r.pay,r.receive,r.fixed_receive,r.partial,r.size,r.cap]);
  const directRows=new Map(direct.cases.map(r=>[identity(r),r]));
  assert.equal(directRows.size,direct.cases.length);
  const perBook=new Map();
  for(const row of protectedRun.cases) {
    const id=identity(row), other=directRows.get(id), module=protectedRun.modules[row.book];
    assert(directRows.delete(id) && other.stage===row.stage);
    const entry=card.books.find(b=>b.book===row.book);
    assert(entry && module.book===entry.book && module.version===entry.version);
    assert.equal(module.runtime,entry.runtimeKeccak);
    assert.equal('0x'+module.selector,entry.takeSelector);
    assert.equal(row.receive,inventory.tokens.USDC);
    const asset=row.pay===inventory.tokens.WETH ? 'WETH' : 'WLTH';
    if(asset==='WLTH') assert.equal(row.pay,'0x99b2b1a2adb02b38222adcd057783d7e5d1fcc7d');
    const paid=row.stage==='paid';
    let reason=null;
    if(paid) {
      assert.equal(other.target,entry.book);
      assert.equal(row.target,module.address);
      assert.equal(row.action.toLowerCase(),other.action.toLowerCase());
      assert.equal(row.action.slice(0,10),entry.takeSelector);
      assert.deepEqual([row.paid,row.net],[other.paid,other.net]);
      for(const r of [row,other]) {
        assert(BigInt(r.paid)>0n && BigInt(r.net)>0n);
        assert.equal(BigInt(r.before[0])-BigInt(r.after[0]),BigInt(r.paid));
        assert.equal(BigInt(r.after[1])-BigInt(r.before[1]),BigInt(r.net));
      }
      assert.deepEqual(row.before.slice(2),row.after.slice(2));
      if(asset==='WLTH') assert(BigInt(row.paid)<=BigInt(funding.after[1])-BigInt(funding.before[1]));
    } else {
      assert.equal(row.stage,'revert');
      assert.equal(row.error.data,'0xa33b1260');
      const error=other.error.data;
      const text=Buffer.from('min ratio').toString('hex');
      if(error==='0x08c379a0'+'20'.padStart(64,'0')+'9'.padStart(64,'0')+text.padEnd(64,'0')) reason='min ratio';
      else {
        assert.equal(error,'0x4e487b71'+'32'.padStart(64,'0'));
        reason='Panic(0x32)';
      }
    }
    const status={asset,input:row.pay,output:row.receive,paid,reason};
    const prior=perBook.get(entry.book);
    if(prior) assert.deepEqual(prior.status,status);
    const sizes=prior?.sizes??new Set();
    sizes.add(row.size);perBook.set(entry.book,{status,sizes});
    entry.sale=status;
    entry.settlementVerified=paid || entry.settlementVerified===true;
  }
  assert.equal(directRows.size,0);
  for(const {sizes} of perBook.values()) assert(sizes.size>=2);
}
