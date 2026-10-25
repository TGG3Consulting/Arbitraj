// Файл: tests/recorders/record_service_tax.js
// Что делает: запись ЖИВОГО ответа узла BSC на пробу переводов USDT (решение 67, check/service_tax.rs): один запрос
//   eth_simulateV1 из tests/data/service_tax_2026-10-25/calls.json (собран кодом бота — service_tax::calls), блок —
//   последний (номер закрепляется в записи). Итог — {about, block, request, response, decimals} в window.__rec; сохранить как
//   tests/data/service_tax_2026-10-25/responses.json.
// Кто вызывает: человек — вставить в консоль браузера, CALLS — содержимое calls.json.
// От чего зависит: ничего (fetch).
// Настройки: нет.

(async () => {
  const NODE = 'https://bsc-dataseed.bnbchain.org';
  const CALLS = /* вставить сюда содержимое calls.json */ null;
  const rpc = async (body) =>
    (await fetch(NODE, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })).json();
  const block = (await rpc({ jsonrpc: '2.0', id: 1, method: 'eth_blockNumber', params: [] })).result;
  const request = JSON.parse(JSON.stringify(CALLS.request));
  request.params[1] = block;
  const response = await rpc(request);
  // decimals() USDT (селектор 313ce567) — знаки для разбора.
  const usdt = request.params[0].blockStateCalls[0].calls[0].to;
  const decimals = await rpc({ jsonrpc: '2.0', id: 2, method: 'eth_call', params: [{ to: usdt, data: '0x313ce567' }, block] });
  window.__rec = { about: CALLS.about, block, request, response, decimals };
  console.log(JSON.stringify(window.__rec));
})();
