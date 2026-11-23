import assert from 'node:assert/strict';
import { test } from 'node:test';
import { executorFeatures, requestExecutorQuote } from '../src/lib/walletExecutor.js';

test('native and singleton support follow the verified runtime version', () => {
  for (const [version, native, singleton] of [
    [null, false, false], ['v1', false, false], ['v2', true, false],
    ['v3', true, true], ['v4', true, true], ['v5', true, true], ['v6', false, false],
  ]) {
    const row = { executor_version: version, executor_wrapped: '0xAB', native_wrapped: '0xab' };
    assert.deepEqual(executorFeatures(row), { native, nativeOk: native, singleton }, String(version));
    assert.equal(executorFeatures({ ...row, native_wrapped: '0xcd' }).nativeOk, false);
    assert.equal(executorFeatures({ ...row, executor_wrapped: null }).nativeOk, false);
    assert.equal(executorFeatures({ ...row, native_wrapped: null }).nativeOk, false);
  }
});

test('confirmation keeps the quoted venue, wallet and previous executor after a wallet refresh', async () => {
  let row = { venue: 'arb_dex', wallet: 'w12', executor: null };
  const pending = requestExecutorQuote(row.venue, row.wallet, row.executor, async (url) => {
    assert.equal(url, '/api/venues/arb_dex/executor/quote?wallet=w12');
    row = { venue: 'bsc_dex', wallet: 'w1', executor: '0xnew' };
    return { bytecode_sha256: 'hash', wnative: 'wrapped' };
  });
  const quote = await pending;
  assert.equal(quote.deployUrl, '/api/venues/arb_dex/executor/deploy?wallet=w12');
  assert.equal(quote.previousExecutor, null);
  assert.equal(quote.bytecode_sha256, 'hash');
});
