// Verify Base V6 from the CI compiler template and the archived protected Raindex settlement.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import sha3 from 'js-sha3';

const [mode, compiledPath, cardPath] = process.argv.slice(2);
assert(['--capture', '--check'].includes(mode) && compiledPath && cardPath);
const compiled = JSON.parse(readFileSync(compiledPath));
const archive = name => JSON.parse(gunzipSync(readFileSync(
  `tests/data/first_run_dex_2026-11-26/raindex_base_${name}.json.gz`)));
const directory = archive('quotes');
const evidence = archive('protected_wallets');
const [book] = Object.values(evidence.modules);
assert.equal(Object.keys(evidence.modules).length, 1);
assert.equal(evidence.chain, 8453);
assert.equal(evidence.height, directory.height);
assert.deepEqual(evidence.selector, directory.anchor);
assert.equal(directory.anchor.requireCanonical, true);
assert.equal(book.book, directory.book);
assert.equal(book.version, 6);
assert.equal(book.selector, '69c72856');
assert.equal(new URL(directory.subgraph).protocol, 'https:');
const constructor = compiled.abi.find(item => item.type === 'constructor');
assert.deepEqual(constructor.inputs.map(input => input.type), ['address', 'bytes32', 'bytes4']);
const words = [book.book.slice(2).padStart(64, '0'), directory.runtime.slice(2), book.selector.padEnd(64, '0')]
  .map(hex => Buffer.from(hex, 'hex'));
const init = Buffer.concat([Buffer.from(compiled.creation, 'hex'), ...words]);
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

// Solidity keys immutableReferences by AST IDs. The recorded on-chain constructor result
// identifies the mapping independently of AST numbering or object-key order.
const matches = [];
for (const order of [[0,1,2],[0,2,1],[1,0,2],[1,2,0],[2,0,1],[2,1,0]]) {
  const runtime = Buffer.from(template);
  for (const [i, group] of groups.entries())
    for (const {start} of group) words[order[i]].copy(runtime, start);
  if (keccak(runtime) === book.runtime) matches.push(runtime);
}
assert.equal(matches.length, 1, 'compiled runtime must reproduce the recorded Base constructor');
const [runtime] = matches;
const card = {
  kind: 'raindex', chainId: evidence.chain, sourceBlock: evidence.selector.blockHash,
  creation: '0x' + compiled.creation, solc: compiled.solc, runtimeBytes: runtime.length,
  books: [{
    version: book.version, book: book.book, bookHash: directory.runtime, takeSelector: '0x' + book.selector,
    runtimeKeccak: keccak(runtime), runtimeSha256: sha256(runtime), creationSha256: sha256(init),
    sourceKind: 'graphql', url: directory.subgraph
  }]
};

if (mode === '--check') {
  assert.deepEqual(JSON.parse(readFileSync(cardPath)), card, 'Raindex deployment card differs from the compiled constructor');
} else {
  // Only public canonical reads. The CREATE eth_call checks all constructor arguments against the actual book at H.
  async function confirm(endpoint) {
    async function rpc(method, params) {
      const response = await fetch(endpoint, {
        method: 'POST', headers: {'Content-Type':'application/json'}, signal: AbortSignal.timeout(18000),
        body: JSON.stringify({jsonrpc:'2.0', id:1, method, params})
      });
      if (!response.ok) throw Error('HTTP ' + response.status);
      const {result, error} = await response.json();
      if (error || result == null) throw Error(error?.message || 'missing result');
      return result;
    }
    assert.equal(Number(await rpc('eth_chainId', [])), evidence.chain);
    const code = await rpc('eth_getCode', [book.book, directory.anchor]);
    assert.equal(keccak(Buffer.from(code.slice(2), 'hex')), directory.runtime);
    const created = await rpc('eth_call', [{from:evidence.payer, data:'0x'+init.toString('hex')}, directory.anchor]);
    assert.equal(created.toLowerCase(), '0x' + runtime.toString('hex'));
  }
  let confirmed = false;
  for (const endpoint of ['https://base.gateway.tenderly.co','https://mainnet.base.org','https://base-rpc.publicnode.com']) {
    try { await confirm(endpoint); confirmed = true; break; }
    catch (error) { console.error(endpoint, String(error.message).slice(0,180)); }
  }
  assert(confirmed, 'no canonical constructor confirmation');
  writeFileSync(cardPath, JSON.stringify(card) + '\n');
}

