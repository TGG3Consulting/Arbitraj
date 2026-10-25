# tests/data/receipts_batch_2026-10-25 — заголовок и квитанции блока одним JSON-RPC batch (решение 69)

`calls.json` — что записывать; рекордер — `tests/recorders/record_receipts_batch.js` (консоль браузера).

`responses.json` — живая запись публичного узла `https://bsc-dataseed.bnbchain.org` (техлид, 2026-10-25, файлы по коммиту
5af51fb, sha256 сверены): latest 129761642; batch [eth_getBlockByNumber(n,false), eth_getBlockReceipts(n)] для блоков
129761639 (702 мс, 581 КБ), 129761640 (356 мс, 172 КБ), 129761641 (351 мс, 166 КБ) и два блока одним POST — 4 вызова
(400 мс, 753 КБ). Узел batch принимает, во всех ответах — массив по id.
