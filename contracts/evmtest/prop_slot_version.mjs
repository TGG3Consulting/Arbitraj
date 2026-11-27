// Офлайн-проверка для CI. Вызывает slotVersion() в локальной EVM на байткоде модулей
// Kaliber и Taurus из карточек и сверяет ABI-ответ, который использует Rust при выборе модуля.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {gunzipSync} from 'node:zlib';
import {Interface, getBytes, hexlify, keccak256} from 'ethers';
import {addr, chain} from './lib.mjs';

const fixtures = new URL('../../tests/data/first_run_dex_2026-11-26/', import.meta.url);
const assets = new URL('../../ui/src/assets/', import.meta.url);
const recorded = name => JSON.parse(gunzipSync(readFileSync(new URL(name + '.json.gz', fixtures))));
const card = name => JSON.parse(readFileSync(new URL(name + '.json', assets), 'utf8'));
const legacy = recorded('prop_slot_runtime_v1');
const current = recorded('prop_slot_creation_v2');
const getter = new Interface(['function slotVersion() view returns (uint256)']);
const data = getBytes(getter.encodeFunctionData('slotVersion'));
const versions = [
  [card('prop-slot-venue-ethereum'), market => legacy.markets[market], 'v1', 'bound_wallet'],
  [card('prop-slot-v2-ethereum-previous'), market => current.markets[market].previousRuntime, 'v2', 'bound_v2'],
  [card('prop-slot-v2-ethereum'), market => current.markets[market].currentRuntime, 'v2', null],
];
assert.equal(legacy.sourceBlock, current.verification.sourceBlock);

for (const name of ['kaliber', 'taurus']) {
  const {owner, module} = recorded(name + '_ethereum_wallets');
  const bound = recorded(name + '_ethereum_bound_v2').builders;
  const market = bound[0].market;
  for (const [manifest, runtimeFor, generation, archive] of versions) {
    const runtime = runtimeFor(market);
    const hash = keccak256(runtime);
    assert.equal(manifest.chainId, 1);
    assert.equal(manifest.sourceBlock, legacy.sourceBlock);
    assert.equal(hash, manifest.markets[market].runtimeKeccak);
    if (archive) {
      const {builders} = recorded(name + '_ethereum_' + archive);
      assert.ok(builders.length && builders.every(b => b.market === market && b.runtime === hash));
    }
    const {evm, sm} = await chain();
    await sm.putContractCode(addr(module), getBytes(runtime));
    const {execResult} = await evm.runCall({
      caller: addr(owner), origin: addr(owner), to: addr(module), data, isStatic: true,
    });
    assert.ifError(execResult.exceptionError);
    assert.equal(hexlify(execResult.returnValue), legacy.expectedSlotVersionReturns[generation]);
    console.log(name, hash, 'slotVersion()', getter.decodeFunctionResult('slotVersion', execResult.returnValue)[0].toString());
  }
}
