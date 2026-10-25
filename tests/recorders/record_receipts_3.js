// Файл: tests/recorders/record_receipts_3.js
// Что делает: запись ЖИВЫХ квитанций блоков BSC для решения 68 (события пулов из eth_getBlockReceipts по HTTP):
//   1) eth_blockNumber — последний блок L;
//   2) eth_getBlockReceipts по блокам L−SCAN+1 … L — поиск окна: WINDOW подряд идущих блоков с наибольшим числом логов
//      менеджеров Infinity CL / Uniswap V4 с ЛЮБЫМ PoolId в topics[1] (при равенстве — логов наших пулов); в записи —
//      seen: менеджер, PoolId и topic0 всех логов менеджеров окна (тесты берут эти PoolId как «наши»);
//   3) eth_getBlockByNumber(n, false) для блоков окна и блока перед окном — hash и parentHash (проверка цепочки и реорга);
//   4) если в скане есть блок со событием создания пула (topic0 из created_topics) — его квитанции и заголовок тоже.
//   Итог — {about, node, latest, window, created, seen, results: [{name, request, response}]} в window.__rec; сохранить как
//   tests/data/receipts_2026-10-25/responses_3.json.
// Кто вызывает: человек — вставить в консоль браузера; CALLS — содержимое tests/data/receipts_2026-10-25/calls_3.json.
// От чего зависит: ничего (fetch).
// Настройки: SCAN, WINDOW — из calls.json (только объём записи, на данные не влияют).

(async () => {
  const CALLS = /* вставить сюда содержимое calls_3.json */ null;
  const NODE = CALLS.node;
  let id = 0;
  const rpc = async (method, params) => {
    const request = { jsonrpc: '2.0', id: ++id, method, params };
    const response = await (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(request) })).json();
    return { request, response };
  };
  const hex = (n) => '0x' + n.toString(16);
  const pools = new Set(CALLS.pools.map((p) => p.addr.toLowerCase()));
  const managers = new Set(CALLS.managers.map((a) => a.toLowerCase()));
  const ids = new Set(CALLS.pool_ids.map((p) => p.id.toLowerCase()));
  const created = new Set(Object.values(CALLS.created_topics).map((t) => t.toLowerCase()));
  const single = (l) => managers.has(l.address.toLowerCase()) && !!l.topics[1];
  const ours = (l) => pools.has(l.address.toLowerCase()) || (managers.has(l.address.toLowerCase()) && l.topics[1] && ids.has(l.topics[1].toLowerCase()));
  const latest = parseInt((await rpc('eth_blockNumber', [])).response.result, 16);
  const scan = [];
  for (let n = latest - CALLS.scan + 1; n <= latest; n++) {
    const r = await rpc('eth_getBlockReceipts', [hex(n)]);
    const logs = (r.response.result || []).flatMap((x) => x.logs);
    scan.push({ n, r, mine: logs.filter(ours).length, single: logs.filter(single).length, made: logs.some((l) => l.topics[0] && created.has(l.topics[0].toLowerCase())) });
  }
  let best = 0;
  for (let i = 1; i + CALLS.window <= scan.length; i++) {
    const s = (k) => scan.slice(k, k + CALLS.window).reduce((a, b) => a + b.single, 0);
    const m = (k) => scan.slice(k, k + CALLS.window).reduce((a, b) => a + b.mine, 0);
    if (s(i) > s(best) || (s(i) === s(best) && m(i) > m(best))) best = i;
  }
  const win = scan.slice(best, best + CALLS.window);
  const made = scan.find((x) => x.made && !win.includes(x));
  const results = [];
  const header = async (n) => {
    const h = await rpc('eth_getBlockByNumber', [hex(n), false]);
    results.push({ name: `header ${n}`, ...h });
  };
  await header(win[0].n - 1);
  for (const x of win) {
    await header(x.n);
    results.push({ name: `receipts ${x.n} (наших логов ${x.mine}, менеджеров ${x.single})`, ...x.r });
  }
  if (made) {
    await header(made.n);
    results.push({ name: `receipts ${made.n} (создание пула)`, ...made.r });
  }
  const seen = [];
  const key = new Set();
  for (const x of win) {
    for (const l of x.r.response.result.flatMap((t) => t.logs).filter(single)) {
      const k = [l.address.toLowerCase(), l.topics[1].toLowerCase(), l.topics[0].toLowerCase()].join(' ');
      if (!key.has(k)) {
        key.add(k);
        seen.push({ manager: l.address.toLowerCase(), id: l.topics[1].toLowerCase(), topic0: l.topics[0].toLowerCase() });
      }
    }
  }
  window.__rec = { about: CALLS.about, node: NODE, latest, window: win.map((x) => x.n), created: made ? made.n : null, seen, results };
  console.log('окно', win.map((x) => [x.n, x.mine, x.single]), 'создание', made && made.n);
  console.log('сохраните JSON.stringify(window.__rec) как responses.json');
})();
