// Файл: tests/recorders/record_depth.js
// Что делает: запись для решения 73 (глубина пулов по направлениям в полосе цены). Запускается в консоли браузера:
//   0) GeckoTerminal: пулы каждой монеты calls.json (страница 1) — ответ целиком (тест разбирает его gt_parse, как бот);
//   1) eth_blockNumber публичного узла — блок закрепляется для всех вызовов;
//   2) tickSpacing пулов: V3 — tickSpacing(); Infinity CL — CLPoolManager.poolIdToPoolKey(id) (parameters, биты 16–39);
//      Uniswap V4 — PositionManager.poolKeys(bytes25 id) (4-е слово);
//   3) состояние: V2 getReserves; V3 slot0 + liquidity; Infinity CL getSlot0 + getLiquidity; V4 StateView getSlot0 +
//      getLiquidity;
//   4) слова битмапа ±tick_words вокруг текущего тика: V3 — TickLens getPopulatedTicksInWord, Infinity CL —
//      getPoolBitmapInfo, V4 — StateView getTickBitmap;
//   5) данные установленных тиков CL: getPoolTickInfo / getTickLiquidity;
//   6) balanceOf USDT у пулов монет из balance_of.
//   Каждый вызов — отдельный eth_call на закреплённом блоке. Итог — {about, block, gecko, pools, results} в window.__rec;
//   сохранить как tests/data/depth_2026-10-25/responses.json.
// Кто вызывает: человек — вставить в консоль; CALLS — содержимое tests/data/depth_2026-10-25/calls.json.
// От чего зависит: ничего (fetch). Селекторы — из sol!-описаний бота (pools/abi_v2v3.rs, abi_v4.rs) и записи
//   tests/data/indexers_2026-10-24/arb_node_responses_2026-10-24.json (poolIdToPoolKey 0x0e2d484a, poolKeys 0x86b6be7d,
//   tickSpacing 0xd0c93a7c).
// Настройки: tick_words — только объём записи (у теста — полоса из dex.depth_band_pct внутри записанного).

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
  const rpc = async (method, params) =>
    (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: ++seq, method, params }) })).json();
  const results = [];
  const got = {};
  const block = (await rpc('eth_blockNumber', [])).result;
  const call = async (name, to, input) => {
    const request = { name, method: 'eth_call', params: [{ to, input }, block] };
    const response = await rpc('eth_call', request.params);
    if (typeof response.result === 'string') got[name] = strip(response.result);
    results.push({ name, request, response });
    return got[name];
  };
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
  // 2–3) tickSpacing и состояние.
  for (const p of pools) {
    if (p.kind === 'v2') { await call(`${p.tag} getReserves`, p.addr, '0x' + SEL.reserves); continue; }
    if (p.kind === 'pcs_v3' || p.kind === 'uni_v3') {
      const ts = await call(`${p.tag} tickSpacing`, p.addr, '0x' + SEL.tickSpacing);
      p.spacing = ts ? Number(signed256(ts.slice(0, 64))) : null;
      await call(`${p.tag} slot0`, p.addr, '0x' + SEL.slot0);
      await call(`${p.tag} liquidity`, p.addr, '0x' + SEL.liquidity);
      p.head = `${p.tag} slot0`;
    } else if (p.kind === 'infinity_cl') {
      const k = await call(`${p.tag} CLPoolManager poolIdToPoolKey`, C.cl_manager, '0x' + SEL.poolIdToPoolKey + word(p.id));
      p.spacing = k ? Number(signed24((BigInt('0x' + k.slice(5 * 64, 6 * 64)) >> 16n) & 0xffffffn)) : null;
      await call(`${p.tag} CLPoolManager getSlot0`, C.cl_manager, '0x' + SEL.getSlot0 + word(p.id));
      await call(`${p.tag} CLPoolManager getLiquidity`, C.cl_manager, '0x' + SEL.getLiquidity + word(p.id));
      p.head = `${p.tag} CLPoolManager getSlot0`;
    } else {
      const k = await call(`${p.tag} PositionManager poolKeys`, C.position_manager, '0x' + SEL.poolKeys + strip(p.id).slice(0, 50).padEnd(64, '0'));
      p.spacing = k ? Number(signed256(k.slice(3 * 64, 4 * 64))) : null;
      await call(`${p.tag} StateView getSlot0`, C.state_view, '0x' + SEL.getSlot0 + word(p.id));
      await call(`${p.tag} StateView getLiquidity`, C.state_view, '0x' + SEL.getLiquidity + word(p.id));
      p.head = `${p.tag} StateView getSlot0`;
    }
  }
  // 4–5) слова битмапа и тики.
  for (const p of pools) {
    if (p.kind === 'v2' || !p.spacing || !got[p.head]) continue;
    const tick = Number(signed256(got[p.head].slice(64, 128)));
    const w = Math.floor(Math.floor(tick / p.spacing) / 256);
    p.words = [w - CALLS.tick_words, w + CALLS.tick_words];
    const ticks = [];
    for (let k = p.words[0]; k <= p.words[1]; k++) {
      if (p.kind === 'pcs_v3' || p.kind === 'uni_v3') {
        await call(`${p.tag} TickLens word ${k}`, p.kind === 'pcs_v3' ? C.pcs_tick_lens : C.uni_tick_lens, '0x' + SEL.lens + word(p.addr) + int256(k));
        continue;
      }
      const name = p.kind === 'infinity_cl' ? `${p.tag} CLPoolManager getPoolBitmapInfo ${k}` : `${p.tag} StateView getTickBitmap ${k}`;
      const to = p.kind === 'infinity_cl' ? C.cl_manager : C.state_view;
      const sel = p.kind === 'infinity_cl' ? SEL.bitmapInf : SEL.bitmapV4;
      const v = BigInt('0x' + ((await call(name, to, '0x' + sel + word(p.id) + int256(k))) || '0'));
      for (let b = 0; b < 256; b++) if ((v >> BigInt(b)) & 1n) ticks.push((k * 256 + b) * p.spacing);
    }
    for (const t of ticks) {
      if (p.kind === 'infinity_cl') await call(`${p.tag} CLPoolManager getPoolTickInfo ${t}`, C.cl_manager, '0x' + SEL.tickInf + word(p.id) + int256(t));
      else await call(`${p.tag} StateView getTickLiquidity ${t}`, C.state_view, '0x' + SEL.tickV4 + word(p.id) + int256(t));
    }
  }
  // 6) balanceOf USDT пула.
  for (const p of pools) {
    if (CALLS.balance_of.includes(p.coin) && p.addr) await call(`${p.tag} USDT balanceOf`, CALLS.usdt, '0x' + SEL.balanceOf + word(p.addr));
  }
  const errors = results.filter((x) => x.response.error).length;
  window.__rec = { about: CALLS.about, node: NODE, block, gecko, pools, results };
  console.log(`блок ${parseInt(block, 16)}: пулов ${pools.length}, вызовов ${results.length}, ошибок ${errors}`, pools.map((p) => [p.tag, p.kind, p.spacing]));
  console.log('сохраните JSON.stringify(window.__rec) как responses.json');
})();
