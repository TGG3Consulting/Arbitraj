// Файл: tests/recorders/record_elastic.js
// Что делает: запись состояния ОДНОГО пула KyberSwap Elastic (решение 81) на ОДНОМ блоке для теста котировки:
//   getPoolState (цена, тик), getLiquidityState (baseL, reinvestL), swapFeeUnits, tickDistance; все инициализированные
//   тики связного списка через TicksFeesReader.getTicksInRange(pool, MIN_TICK, 0) и liquidityGross/Net каждого тика
//   методом пула ticks(); реестр фабрики getPool(token0, token1, swapFeeUnits); и для сверки (решение 65) —
//   QuoterV2.quoteExactInputSingle по каждому probe. Запускается в консоли браузера; ключ не нужен (узел публичный).
//   Ничего не торгует — только читает и складывает в window.__rec.
//
// Источники (официальные, решение 23): KyberNetwork/ks-elastic-sc — IPoolStorage (getPoolState/getLiquidityState/
//   swapFeeUnits/tickDistance/ticks), TicksFeesReader.getTicksInRange, IFactory.getPool, периферийный QuoterV2.
//   Официальные адреса BSC v1 (README ks-elastic-sc): Factory 0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a,
//   QuoterV2 0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f. Адрес TicksFeesReader в BSC — из официального деплоя
//   (deployment/*.json ks-elastic-sc) — подставить в CALLS.tick_reader.
//
// Как запустить:
//   1) открыть вкладку (например https://bnbchain.org), F12 → Console;
//   2) подставить содержимое calls.json в CALLS ниже (адреса пула/фабрики/квотера/читателя, токены, probes);
//   3) вставить файл, Enter; дождаться строки-итога;
//   4) copy(JSON.stringify(window.__rec)) и сохранить как tests/data/elastic_<дата>/responses.json.
//
// Форма CALLS (calls.json), плейсхолдеры подставить:
//   {
//     "about": "пул MONETA-USDT KyberSwap Elastic, один блок",
//     "node": "<HTTPS JSON-RPC BNB Chain>",
//     "pool": "0x<адрес пула Elastic>",
//     "factory": "0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a",
//     "quoter": "0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f",
//     "tick_reader": "0x<TicksFeesReader BSC>",
//     "token0": "0x<token0 пула по сортировке адресов>",
//     "token1": "0x<token1 пула>",
//     "batch_max": 50,
//     "probes": [
//       { "amount_in": "1000000000000000000", "zero_for_one": true },
//       { "amount_in": "1000000000000000000000", "zero_for_one": false }
//     ]
//   }

(async () => {
  const CALLS = /* вставить сюда содержимое calls.json */ null;
  const NODE = CALLS.node;
  const SEL = {
    getPoolState: '217ac237', getLiquidityState: 'ab612f2b', swapFeeUnits: 'c79a590e',
    tickDistance: '48626a8c', ticks: 'f30dba93', getTicksInRange: '6bea4865',
    getPool: '1698ee82', quote: 'c6a5026a',
  };
  const MIN_TICK = -887272n;
  const MAX_TICK = 887272n;
  const MASK = (1n << 256n) - 1n;
  const strip = (h) => h.replace(/^0x/, '').toLowerCase();
  const word = (h) => strip(h).padStart(64, '0');
  const uint = (n) => (BigInt(n) & MASK).toString(16).padStart(64, '0');
  const int = (n) => (BigInt(n) & MASK).toString(16).padStart(64, '0'); // two's complement, 32 байта
  const signed = (h) => { const v = BigInt('0x' + h); return v >= 1n << 255n ? v - (1n << 256n) : v; };
  let seq = 0;
  const results = [];
  const got = {};
  const batch = async (reqs) => {
    for (let i = 0; i < reqs.length; i += CALLS.batch_max) {
      const part = reqs.slice(i, i + CALLS.batch_max).map((r) => ({ ...r, id: ++seq }));
      const body = JSON.stringify(part.map((r) => ({ jsonrpc: '2.0', id: r.id, method: r.method, params: r.params })));
      let arr;
      try { arr = await (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body })).json(); }
      catch (e) { arr = []; }
      const byId = new Map((Array.isArray(arr) ? arr : []).map((x) => [x.id, x]));
      for (const r of part) {
        const response = byId.get(r.id) || { error: { message: 'нет ответа в batch' } };
        if (typeof response.result === 'string') got[r.name] = strip(response.result);
        results.push({ name: r.name, request: { method: r.method, params: r.params }, response });
      }
    }
  };
  const call = (name, to, input, block) => ({ name, method: 'eth_call', params: [{ to, input }, block] });

  const P = CALLS.pool, Q = CALLS.quoter, F = CALLS.factory, R = CALLS.tick_reader;
  // Один блок на всю запись.
  const bn = await (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: ++seq, method: 'eth_blockNumber', params: [] }) })).json();
  const block = bn.result;

  // 1) головы пула.
  await batch([
    call('getPoolState', P, '0x' + SEL.getPoolState, block),
    call('getLiquidityState', P, '0x' + SEL.getLiquidityState, block),
    call('swapFeeUnits', P, '0x' + SEL.swapFeeUnits, block),
    call('tickDistance', P, '0x' + SEL.tickDistance, block),
  ]);
  const feeUnits = BigInt('0x' + (got.swapFeeUnits || '0'));

  // 2) реестр фабрики getPool(token0, token1, swapFeeUnits) + все тики списка через читатель.
  await batch([
    call('getPool', F, '0x' + SEL.getPool + word(CALLS.token0) + word(CALLS.token1) + uint(feeUnits), block),
    call('getTicksInRange', R, '0x' + SEL.getTicksInRange + word(P) + int(MIN_TICK) + uint(0n), block),
  ]);

  // 3) разбор массива тиков: [offset][len][tick0][tick1]... (int24, sign-extended 32 байта); до MAX_TICK включительно.
  const raw = got.getTicksInRange || '';
  const ticks = [];
  if (raw.length >= 128) {
    const len = Number(BigInt('0x' + raw.slice(64, 128)));
    for (let i = 0; i < len; i++) {
      const w = raw.slice(128 + i * 64, 128 + (i + 1) * 64);
      if (w.length < 64) break;
      const t = signed(w);
      ticks.push(t.toString());
      if (t >= MAX_TICK) break;
    }
  }

  // 4) ticks(t) по каждому тику — liquidityGross/liquidityNet.
  await batch(ticks.map((t) => call(`ticks ${t}`, P, '0x' + SEL.ticks + int(BigInt(t)), block)));

  // 5) сверка: QuoterV2.quoteExactInputSingle по каждому probe.
  const probes = CALLS.probes || [];
  await batch(probes.map((pr, i) => {
    const tin = pr.zero_for_one ? CALLS.token0 : CALLS.token1;
    const tout = pr.zero_for_one ? CALLS.token1 : CALLS.token0;
    const params = word(tin) + word(tout) + uint(pr.amount_in) + uint(feeUnits) + uint(0n);
    return call(`quote ${i}`, Q, '0x' + SEL.quote + params, block);
  }));

  window.__rec = {
    about: CALLS.about, node: NODE, block,
    pool: P, factory: F, quoter: Q, tick_reader: R,
    token0: CALLS.token0, token1: CALLS.token1,
    fee_units: feeUnits.toString(),
    ticks, probes, responses: got, results,
  };
  console.log(`Elastic: блок ${parseInt(block, 16)}, тиков ${ticks.length}, probes ${probes.length}. copy(JSON.stringify(window.__rec))`);
})();
