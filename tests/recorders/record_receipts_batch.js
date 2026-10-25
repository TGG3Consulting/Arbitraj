// Файл: tests/recorders/record_receipts_batch.js
// Что делает: запись ЖИВОГО ответа публичного узла BNB Chain на JSON-RPC batch (решение 69): заголовок и квитанции
//   блока одним HTTP-запросом — [eth_getBlockByNumber(n, false), eth_getBlockReceipts(n)] для трёх подряд блоков
//   (L−3 … L−1, на шаг позади головы — блоки точно есть у узла), и один batch из двух блоков сразу (4 вызова).
//   Итог — {about, node, latest, results: [{name, request, response, ms, bytes}]} в window.__rec; сохранить как
//   tests/data/receipts_batch_2026-10-25/responses.json. ms и bytes — время и размер ответа (для пределов).
// Кто вызывает: человек — вставить в консоль браузера; CALLS — содержимое tests/data/receipts_batch_2026-10-25/calls.json.
// От чего зависит: ничего (fetch).
// Настройки: нет.

(async () => {
  const CALLS = /* вставить сюда содержимое calls.json */ null;
  const NODE = CALLS.node;
  const post = async (body) => {
    const t = performance.now();
    const text = await (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })).text();
    return { response: JSON.parse(text), ms: Math.round(performance.now() - t), bytes: text.length };
  };
  const hex = (n) => '0x' + n.toString(16);
  const pair = (n, id) => [
    { jsonrpc: '2.0', id, method: 'eth_getBlockByNumber', params: [hex(n), false] },
    { jsonrpc: '2.0', id: id + 1, method: 'eth_getBlockReceipts', params: [hex(n)] },
  ];
  const latest = parseInt((await post({ jsonrpc: '2.0', id: 1, method: 'eth_blockNumber', params: [] })).response.result, 16);
  const results = [];
  for (let n = latest - CALLS.blocks; n < latest; n++) {
    const request = pair(n, 0);
    results.push({ name: `batch ${n}`, request, ...(await post(request)) });
  }
  const two = [...pair(latest - CALLS.blocks, 0), ...pair(latest - CALLS.blocks + 1, 2)];
  results.push({ name: `batch ${latest - CALLS.blocks} ${latest - CALLS.blocks + 1}`, request: two, ...(await post(two)) });
  window.__rec = { about: CALLS.about, node: NODE, latest, results };
  console.log(results.map((r) => [r.name, Array.isArray(r.response) ? r.response.length : 'не массив', r.ms, r.bytes]));
  console.log('сохраните JSON.stringify(window.__rec) как responses.json');
})();
