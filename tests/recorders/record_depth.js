// Файл: tests/recorders/record_depth.js
// Что делает: запись для решения 73 (глубина пулов по направлениям в полосе цены). Запускается в консоли браузера:
//   0) GeckoTerminal: пулы каждой монеты calls.json (страница 1) — ответ целиком (тест разбирает его gt_parse, как бот);
//   1) на КАЖДЫЙ пул — свой свежий eth_blockNumber, все вызовы пула закреплены на нём (номер — в pools[].block и в
//      params каждого вызова): bsc-dataseed не архивный — состояние блока пропадает примерно через минуту («missing trie
//      node», запись 25.10: 1004 ошибки после вызова №171 при одном блоке на всю запись);
//   2) tickSpacing пулов: V3 — tickSpacing(); Infinity CL — CLPoolManager.poolIdToPoolKey(id) (parameters, биты 16–39);
//      Uniswap V4 — PositionManager.poolKeys(bytes25 id) (4-е слово);
//   3) состояние: V2 getReserves; V3 slot0 + liquidity; Infinity CL getSlot0 + getLiquidity; V4 StateView getSlot0 +
//      getLiquidity;
//   4) слова битмапа ±tick_words вокруг текущего тика: V3 — TickLens getPopulatedTicksInWord, Infinity CL —
//      getPoolBitmapInfo, V4 — StateView getTickBitmap;
//   5) данные установленных тиков CL: getPoolTickInfo / getTickLiquidity;
//   6) balanceOf USDT у пулов монет из balance_of.
//   Вызовы пула — JSON-RPC batch по batch_max (узел принимает batch — tests/data/receipts_batch_2026-10-25), три захода:
//   состояние → слова битмапа → тики (пачки тиков параллельно): пул укладывается в пару секунд. Итог — {about, block, gecko, pools, results} в window.__rec;
//   сохранить как tests/data/depth_2026-10-25/responses.json.
// Кто вызывает: человек — вставить в консоль; CALLS — содержимое tests/data/depth_2026-10-25/calls.json.
// От чего зависит: ничего (fetch). Селекторы — из sol!-описаний бота (pools/abi_v2v3.rs, abi_v4.rs) и записи
//   tests/data/indexers_2026-10-24/arb_node_responses_2026-10-24.json (poolIdToPoolKey 0x0e2d484a, poolKeys 0x86b6be7d,
//   tickSpacing 0xd0c93a7c).
// Настройки: tick_words, batch_max — только объём и нарезка записи (у теста — полоса из dex.depth_band_pct внутри записанного).

(async () => {
  const CALLS = /* вставить сюда содержимое calls.json */ null;
  const NODE = CALLS.node;
  const C = CALLS.contracts;
  const SEL = {
    reserves: '0902f1ac', slot0: '3850c7bd', liquidity: '1a686502', tickSpacing: 'd0c93a7c',
    getSlot0: 'c815641c', getLiquidity: 'fa6793d5', poolIdToPoolKey: '0e2d484a', poolKeys: '86b6be7d',
    lens: '351fb478', bitmapInf: '7c352ef6', bitmapV4: '1c7ccb4c', tickInf: '5aa208a4', tickV4: 'caedab54',
    balanceOf: '70a08231',
  };
  const MASK = (1n << 256n) - 1n;
  const strip = (h) => h.replace(/^0x/, '').toLowerCase();
  const word = (h) => strip(h).padStart(64, '0');
  const int256 = (n) => (BigInt(n) & MASK).toString(16).padStart(64, '0');
  const signed24 = (v) => (v >= 1n << 23n ? v - (1n << 24n) : v);
  const signed256 = (h) => { const v = BigInt('0x' + h); return v >= 1n << 255n ? v - (1n << 256n) : v; };
  let seq = 0;
  const results = [];
  const got = {};
  // JSON-RPC batch: ответы сопоставляются по id (порядок в batch не гарантирован).
  const batch = async (reqs) => {
    const out = [];
    for (let i = 0; i < reqs.length; i += CALLS.batch_max) {
      const part = reqs.slice(i, i + CALLS.batch_max).map((r) => ({ ...r, id: ++seq }));
      out.push((async () => {
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
      })());
    }
    await Promise.all(out);
  };
  const call = (name, to, input, block) => ({ name, method: 'eth_call', params: [{ to, input }, block] });
  // 0) GeckoTerminal.
  const gecko = {};
  const pools = [];
  for (const [coin, addr] of Object.entries(CALLS.coins)) {
    const text = await (await fetch(CALLS.gecko.replace('{address}', addr))).text();
    gecko[coin] = text;
    for (const p of JSON.parse(text).data || []) {
      const dex = p.relationships.dex.data.id;
      const kind = CALLS.kinds[dex];
      if (!kind) continue;
      const a = p.attributes.address.toLowerCase();
      pools.push({ coin, dex, kind, tag: `${coin} ${dex} ${a}`, addr: a.length === 42 ? a : null, id: a.length === 66 ? a : null });
    }
  }
  for (const p of pools) {
    // 1) свой свежий блок пула.
    const bn = await (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: ++seq, method: 'eth_blockNumber', params: [] }) })).json();
    const block = bn.result;
    p.block = block;
    // 2–3) tickSpacing, состояние, balanceOf — первый batch.
    const first = [];
    if (p.kind === 'v2') first.push(call(`${p.tag} getReserves`, p.addr, '0x' + SEL.reserves, block));
    else if (p.kind === 'pcs_v3' || p.kind === 'uni_v3') {
      first.push(call(`${p.tag} tickSpacing`, p.addr, '0x' + SEL.tickSpacing, block));
      first.push(call(`${p.tag} slot0`, p.addr, '0x' + SEL.slot0, block));
      first.push(call(`${p.tag} liquidity`, p.addr, '0x' + SEL.liquidity, block));
      p.head = `${p.tag} slot0`;
    } else if (p.kind === 'infinity_cl') {
      first.push(call(`${p.tag} CLPoolManager poolIdToPoolKey`, C.cl_manager, '0x' + SEL.poolIdToPoolKey + word(p.id), block));
      first.push(call(`${p.tag} CLPoolManager getSlot0`, C.cl_manager, '0x' + SEL.getSlot0 + word(p.id), block));
      first.push(call(`${p.tag} CLPoolManager getLiquidity`, C.cl_manager, '0x' + SEL.getLiquidity + word(p.id), block));
      p.head = `${p.tag} CLPoolManager getSlot0`;
    } else {
      first.push(call(`${p.tag} PositionManager poolKeys`, C.position_manager, '0x' + SEL.poolKeys + strip(p.id).slice(0, 50).padEnd(64, '0'), block));
      first.push(call(`${p.tag} StateView getSlot0`, C.state_view, '0x' + SEL.getSlot0 + word(p.id), block));
      first.push(call(`${p.tag} StateView getLiquidity`, C.state_view, '0x' + SEL.getLiquidity + word(p.id), block));
      p.head = `${p.tag} StateView getSlot0`;
    }
    if (CALLS.balance_of.includes(p.coin) && p.addr) first.push(call(`${p.tag} USDT balanceOf`, CALLS.usdt, '0x' + SEL.balanceOf + word(p.addr), block));
    await batch(first);
    if (p.kind === 'v2') continue;
    const ts = got[p.kind === 'infinity_cl' ? `${p.tag} CLPoolManager poolIdToPoolKey` : p.kind === 'uni_v4' ? `${p.tag} PositionManager poolKeys` : `${p.tag} tickSpacing`];
    if (ts) {
      if (p.kind === 'infinity_cl') p.spacing = Number(signed24((BigInt('0x' + ts.slice(5 * 64, 6 * 64)) >> 16n) & 0xffffffn));
      else if (p.kind === 'uni_v4') p.spacing = Number(signed256(ts.slice(3 * 64, 4 * 64)));
      else p.spacing = Number(signed256(ts.slice(0, 64)));
    }
    if (!p.spacing || !got[p.head]) continue;
    // 4) слова битмапа ±tick_words — второй batch.
    const tick = Number(signed256(got[p.head].slice(64, 128)));
    const w = Math.floor(Math.floor(tick / p.spacing) / 256);
    p.words = [w - CALLS.tick_words, w + CALLS.tick_words];
    const words = [];
    for (let k = p.words[0]; k <= p.words[1]; k++) {
      if (p.kind === 'pcs_v3' || p.kind === 'uni_v3') {
        words.push(call(`${p.tag} TickLens word ${k}`, p.kind === 'pcs_v3' ? C.pcs_tick_lens : C.uni_tick_lens, '0x' + SEL.lens + word(p.addr) + int256(k), block));
      } else if (p.kind === 'infinity_cl') {
        words.push(call(`${p.tag} CLPoolManager getPoolBitmapInfo ${k}`, C.cl_manager, '0x' + SEL.bitmapInf + word(p.id) + int256(k), block));
      } else {
        words.push(call(`${p.tag} StateView getTickBitmap ${k}`, C.state_view, '0x' + SEL.bitmapV4 + word(p.id) + int256(k), block));
      }
    }
    await batch(words);
    if (p.kind === 'pcs_v3' || p.kind === 'uni_v3') continue;
    // 5) данные установленных тиков — третий заход, пачки параллельно.
    const ticks = [];
    for (const r of words) {
      const k = Number(r.name.split(' ').pop());
      const v = BigInt('0x' + (got[r.name] || '0'));
      for (let b = 0; b < 256; b++) if ((v >> BigInt(b)) & 1n) ticks.push((k * 256 + b) * p.spacing);
    }
    await batch(ticks.map((t) => p.kind === 'infinity_cl'
      ? call(`${p.tag} CLPoolManager getPoolTickInfo ${t}`, C.cl_manager, '0x' + SEL.tickInf + word(p.id) + int256(t), block)
      : call(`${p.tag} StateView getTickLiquidity ${t}`, C.state_view, '0x' + SEL.tickV4 + word(p.id) + int256(t), block)));
    console.log(`${p.tag}: блок ${parseInt(block, 16)}, тиков ${ticks.length}`);
  }
  const errors = results.filter((x) => x.response.error).length;
  window.__rec = { about: CALLS.about, node: NODE, gecko, pools, results };
  const trie = results.filter((x) => /missing trie node/.test(JSON.stringify(x.response.error || ''))).length;
  console.log(`пулов ${pools.length}, вызовов ${results.length}, ошибок ${errors} (missing trie node ${trie})`, pools.map((p) => [p.tag, p.kind, p.spacing]));
  console.log('сохраните JSON.stringify(window.__rec) как responses.json');
})();
