// Файл: tests/recorders/record_route.js
// Что делает: запись состояния пулов МАРШРУТА на ОДНОМ блоке для тестов графа-маршрутизатора (решение 80, шаги
//   80.3/80.4 — перечисление путей и сплит). Отличие от record_depth.js: там на каждый пул свой свежий блок (глубина),
//   здесь ВСЕ пулы маршрута закреплены на одном eth_blockNumber — чтобы перечисление и сплит считались по
//   согласованному срезу рынка (цены пулов одного блока). Запускается в консоли браузера, как record_depth.js;
//   ключ не нужен (узел публичный). Ничего не торгует — только читает и складывает в window.__rec.
//
// Что собирает (по каждому пулу из CALLS.pools — те же вызовы и селекторы, что record_depth.js, тем же Multicall3/
//   JSON-RPC batch, но с общим блоком):
//   V2        — getReserves;
//   pcs_v3/uni_v3 — tickSpacing, slot0, liquidity, слова битмапа ±tick_words, TickLens по словам;
//   infinity_cl   — CLPoolManager poolIdToPoolKey/getSlot0/getLiquidity, getPoolBitmapInfo, getPoolTickInfo;
//   uni_v4        — PositionManager poolKeys, StateView getSlot0/getLiquidity/getTickBitmap/getTickLiquidity;
//   (опц.) balanceOf USDT у пула (CALLS.balance_of) — как в record_depth.
//
// Как запустить:
//   1) открыть вкладку (например https://bnbchain.org), F12 → Console;
//   2) подставить содержимое calls.json в CALLS ниже (адреса/PoolId пулов маршрута — плейсхолдеры заменит техлид/Тигран);
//   3) вставить файл, Enter; дождаться строки-итога;
//   4) copy(JSON.stringify(window.__rec)) и сохранить как tests/data/routes_<дата>/responses.json.
//
// Внимание: только официальные публичные узлы BNB Chain (CLAUDE.md, решение 23). Селекторы — из ABI бота
//   (pools/abi_v2v3.rs, abi_v4.rs), те же, что в record_depth.js; разбор записи тестом — его код (gt не нужен:
//   пулы заданы прямо в CALLS, без GeckoTerminal).
//
// Форма CALLS (calls.json), плейсхолдеры подставить:
//   {
//     "about": "маршрут монеты X: прямой (×2 DEX) + 2-хоп (через WBNB) + 3-хоп (через HUB2 и WBNB), один блок",
//     "node": "<HTTPS JSON-RPC BNB Chain>",
//     "usdt": "0x55d398326f99059ff775485246999027b3197955",
//     "contracts": {
//       "cl_manager": "<PancakeSwap Infinity CLPoolManager>",
//       "position_manager": "<Uniswap V4 PositionManager>",
//       "state_view": "<Uniswap V4 StateView>",
//       "pcs_tick_lens": "<PancakeSwap V3 TickLens>",
//       "uni_tick_lens": "<Uniswap V3 TickLens>"
//     },
//     "tick_words": 3,
//     "batch_max": 50,
//     "balance_of": ["<tag пула, у которого снять balanceOf USDT>", "..."],
//     "pools": [
//       { "tag": "X-USDT pcs_v3",  "kind": "pcs_v3", "addr": "0x..." },
//       { "tag": "X-USDT uni_v3",  "kind": "uni_v3", "addr": "0x..." },
//       { "tag": "X-WBNB pcs_v3",  "kind": "pcs_v3", "addr": "0x..." },
//       { "tag": "WBNB-USDT v2",   "kind": "v2",     "addr": "0x..." },
//       { "tag": "X-HUB2 v2",      "kind": "v2",     "addr": "0x..." },
//       { "tag": "HUB2-WBNB v2",   "kind": "v2",     "addr": "0x..." },
//       { "tag": "HUB2 infinity",  "kind": "infinity_cl", "id": "0x..32 bytes PoolId.." }
//     ]
//   }
//   kind: "v2" | "pcs_v3" | "uni_v3" | "infinity_cl" | "uni_v4". Контрактные пулы (V2/V3) задают "addr", менеджерные
//   (infinity_cl/uni_v4) — "id" (PoolId, 32 байта). tick_words — запас слов битмапа вокруг текущего тика (как в depth).

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
  // JSON-RPC batch: ответы по id (порядок в batch не гарантирован). Как record_depth.js.
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

  // ОДИН блок на весь маршрут — ключевое отличие от record_depth.js.
  const bn = await (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: ++seq, method: 'eth_blockNumber', params: [] }) })).json();
  const block = bn.result;
  const pools = CALLS.pools.map((p) => ({ ...p, block }));

  for (const p of pools) {
    // 1) состояние пула + (опц.) balanceOf USDT — первый batch.
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
    if ((CALLS.balance_of || []).includes(p.tag) && p.addr) first.push(call(`${p.tag} USDT balanceOf`, CALLS.usdt, '0x' + SEL.balanceOf + word(p.addr), block));
    await batch(first);
    if (p.kind === 'v2') continue;
    const ts = got[p.kind === 'infinity_cl' ? `${p.tag} CLPoolManager poolIdToPoolKey` : p.kind === 'uni_v4' ? `${p.tag} PositionManager poolKeys` : `${p.tag} tickSpacing`];
    if (ts) {
      if (p.kind === 'infinity_cl') p.spacing = Number(signed24((BigInt('0x' + ts.slice(5 * 64, 6 * 64)) >> 16n) & 0xffffffn));
      else if (p.kind === 'uni_v4') p.spacing = Number(signed256(ts.slice(3 * 64, 4 * 64)));
      else p.spacing = Number(signed256(ts.slice(0, 64)));
    }
    if (!p.spacing || !got[p.head]) continue;
    // 2) слова битмапа ±tick_words вокруг текущего тика — второй batch.
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
    // 3) данные установленных тиков CL/V4 — третий заход.
    const ticks = [];
    for (const r of words) {
      const k = Number(r.name.split(' ').pop());
      const v = BigInt('0x' + (got[r.name] || '0'));
      for (let b = 0; b < 256; b++) if ((v >> BigInt(b)) & 1n) ticks.push((k * 256 + b) * p.spacing);
    }
    await batch(ticks.map((t) => p.kind === 'infinity_cl'
      ? call(`${p.tag} CLPoolManager getPoolTickInfo ${t}`, C.cl_manager, '0x' + SEL.tickInf + word(p.id) + int256(t), block)
      : call(`${p.tag} StateView getTickLiquidity ${t}`, C.state_view, '0x' + SEL.tickV4 + word(p.id) + int256(t), block)));
    console.log(`${p.tag}: тиков ${ticks.length}`);
  }
  const errors = results.filter((x) => x.response.error).length;
  const trie = results.filter((x) => /missing trie node/.test(JSON.stringify(x.response.error || ''))).length;
  window.__rec = { about: CALLS.about, node: NODE, block, pools, results };
  console.log(`маршрут: блок ${parseInt(block, 16)}, пулов ${pools.length}, вызовов ${results.length}, ошибок ${errors} (missing trie node ${trie})`, pools.map((p) => [p.tag, p.kind, p.spacing]));
  console.log('если missing trie node > 0 — повторить сразу (состояние блока у публичного узла живёт ~минуту). Сохраните JSON.stringify(window.__rec) как responses.json');
})();
