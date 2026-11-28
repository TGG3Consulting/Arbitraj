// Bind the compiler template to the active Base books and the recorded settlement outcomes.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { gzipSync, gunzipSync } from 'node:zlib';
import sha3 from 'js-sha3';

const [mode, compiledPath, cardPath, codeV6, codeV5] = process.argv.slice(2);
assert(['--capture', '--check'].includes(mode) && compiledPath && cardPath && codeV6 && codeV5);
const compiled = JSON.parse(readFileSync(compiledPath));
const archive = name => JSON.parse(gunzipSync(readFileSync(
  `tests/data/first_run_dex_2026-11-26/raindex_base_${name}.json.gz`)));
const directory = archive('quotes');
const v6 = archive('protected_wallets');
const v5 = archive('v5_protected_wallets');
const catalog = archive('legacy_directory');
const legacy = catalog.books.find(row => row.version === 5);
assert(legacy);
assert.equal(catalog.height, directory.height);
assert.deepEqual(catalog.anchor, directory.anchor);
assert.equal(directory.anchor.requireCanonical, true);
assert.equal(new URL(directory.subgraph).protocol, 'https:');
const specs = [
  {payout:v6, version:6, address:directory.book, hash:directory.runtime, selector:'69c72856',
   sourceKind:'graphql', source:directory.subgraph, codePath:codeV6},
  {payout:v5, version:5, address:legacy.book, hash:legacy.runtime, selector:'d0f7a862',
   sourceKind:'rpc', source:String(legacy.deployment_start), codePath:codeV5}
];
const constructor = compiled.abi.find(item => item.type === 'constructor');
assert.deepEqual(constructor.inputs.map(input => input.type), ['address', 'bytes32', 'bytes4']);
const template = Buffer.from(compiled.runtime, 'hex');
const groups = Object.values(compiled.immutableReferences);
assert.equal(groups.length, 3);
const positions = new Set();
for (const group of groups) {
  assert(group.length > 0);
  for (const { start, length } of group) {
    assert(length === 32 && start >= 0 && start + length <= template.length);
    assert(template.subarray(start, start + length).equals(Buffer.alloc(length)));
    for (let i = start; i < start + length; ++i) {
      assert(!positions.has(i));
      positions.add(i);
    }
  }
}
const keccak = bytes => '0x' + sha3.keccak256(bytes);
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

const card = {
  kind:'raindex', chainId:8453, sourceBlock:directory.anchor.blockHash,
  creation:'0x'+compiled.creation, solc:compiled.solc, runtimeBytes:template.length, books:[]
};
function verifiedSettlement(spec) {
  const rows = spec.payout.pairs.flatMap(pair => pair.cases.flatMap(c => Object.values(c)))
    .filter(v => v && v.book === spec.address && v.version === spec.version);
  assert(rows.length > 0);
  const paid = rows.some(v => v.stage === 'paid' && BigInt(v.paid) > 0n && BigInt(v.net) > 0n);
  if (spec.version === 6) assert(paid);
  if (spec.version === 5 && !paid) {
    const direct = spec.payout.direct.flatMap(pair => pair.cases.map(c => c.by_output));
    assert.equal(direct.length, rows.length);
    assert(direct.every(v => v.book === spec.address && v.stage === 'revert' &&
      v.error?.data?.startsWith('0xfe720f78')));
    assert(rows.every(v => v.stage === 'revert' && v.error?.data === '0xa33b1260'));
    const audit = archive('v5_transfer_evidence');
    assert.deepEqual(audit.anchor, directory.anchor);
    assert(direct.every(v => audit.tokens[v.receive].isCertificationExpired === true));
  }
  if (spec.version === 5) {
    const independent = archive('v5_holder_wallets');
    assert.equal(independent.height, directory.height);
    assert.deepEqual(independent.selector, directory.anchor);
    assert.equal(independent.book, spec.address);
    assert.equal(independent.runtime, spec.payout.modules[spec.address].runtime);
    const funded = independent.cases.filter(row => row.stage === 'paid');
    assert.equal(new Set(funded.map(row => row.asset)).size, 2);
    for (const row of funded) {
      assert(BigInt(row.paid) > 0n && BigInt(row.net) > 0n);
      assert(row.holder !== spec.payout.payer && row.tight_reproduced && row.excess_minimum_rolled_back);
      assert.equal(BigInt(row.before[0])-BigInt(row.after[0]), BigInt(row.paid));
      assert.equal(BigInt(row.after[1])-BigInt(row.before[1]), BigInt(row.net));
      assert.deepEqual(row.before.slice(2), row.after.slice(2));
    }
    return {settlementVerified:true, projectFundingVerified:paid};
  }
  return {settlementVerified:paid, projectFundingVerified:paid};
}
for (const spec of specs) {
  const evidence = spec.payout;
  const [book] = Object.values(evidence.modules);
  assert.equal(Object.keys(evidence.modules).length, 1);
  assert.equal(evidence.chain, card.chainId);
  assert.equal(evidence.height, directory.height);
  assert.deepEqual(evidence.selector, directory.anchor);
  assert.equal(book.book, spec.address);
  assert.equal(book.version, spec.version);
  assert.equal(book.selector, spec.selector);
  const words = [book.book.slice(2).padStart(64,'0'), spec.hash.slice(2), book.selector.padEnd(64,'0')]
    .map(hex => Buffer.from(hex,'hex'));
  const init = Buffer.concat([Buffer.from(compiled.creation,'hex'), ...words]);
  const matches = [];
  for (const order of [[0,1,2],[0,2,1],[1,0,2],[1,2,0],[2,0,1],[2,1,0]]) {
    const runtime = Buffer.from(template);
    for (const [i, group] of groups.entries())
      for (const {start} of group) words[order[i]].copy(runtime,start);
    if (keccak(runtime) === book.runtime) matches.push(runtime);
  }
  assert.equal(matches.length, 1, 'compiled runtime must reproduce the recorded constructor');
  const [runtime] = matches;
  const verification = verifiedSettlement(spec);
  const entry = {
    version:book.version, book:book.book, bookHash:spec.hash, takeSelector:'0x'+book.selector,
    runtimeKeccak:keccak(runtime), runtimeSha256:sha256(runtime), creationSha256:sha256(init),
    sourceKind:spec.sourceKind, url:spec.source, ...verification
  };
  card.books.push(entry);
  if (mode === '--check') {
    const codes = JSON.parse(gunzipSync(readFileSync(spec.codePath)));
    assert.equal(codes.sourceBlock,card.sourceBlock);
    assert.equal(codes.book,book.book);
    assert.equal(keccak(Buffer.from(codes.bookCode.slice(2),'hex')),spec.hash);
    assert.equal(codes.runtime,'0x'+runtime.toString('hex'));
    continue;
  }
  async function confirm(endpoint) {
    async function rpc(method, params) {
      const response = await fetch(endpoint, {
        method:'POST', headers:{'Content-Type':'application/json'}, signal:AbortSignal.timeout(18000),
        body:JSON.stringify({jsonrpc:'2.0',id:1,method,params})
      });
      if (!response.ok) throw Error('HTTP '+response.status);
      const {result, error} = await response.json();
      if (error || result == null) throw Error(error?.message || 'missing result');
      return result;
    }
    assert.equal(Number(await rpc('eth_chainId',[])),evidence.chain);
    const code = await rpc('eth_getCode',[book.book,directory.anchor]);
    assert.equal(keccak(Buffer.from(code.slice(2),'hex')),spec.hash);
    const created = await rpc('eth_call',[{from:evidence.payer,data:'0x'+init.toString('hex')},directory.anchor]);
    assert.equal(created.toLowerCase(),'0x'+runtime.toString('hex'));
    return {sourceBlock:card.sourceBlock,book:book.book,bookCode:code.toLowerCase(),runtime:created.toLowerCase()};
  }
  let codes;
  for (const endpoint of ['https://base.gateway.tenderly.co','https://mainnet.base.org','https://base-rpc.publicnode.com']) {
    try { codes = await confirm(endpoint); break; }
    catch (error) { console.error(endpoint,String(error.message).slice(0,180)); }
  }
  assert(codes,'no canonical constructor confirmation');
  writeFileSync(spec.codePath,gzipSync(JSON.stringify(codes)));
}
if (mode === '--check') {
  assert.deepEqual(JSON.parse(readFileSync(cardPath)),card,'compiled constructor or recorded settlement changed');
} else {
  writeFileSync(cardPath,JSON.stringify(card)+'\n');
}
