# tests/data/mexc_2026-11-21 — живой ответ REST MEXC 2026-11-21

Кусок ответа из сырого журнала бота на сервере (storage/raw_log.rs, поле `m`), без изменений внутри объектов монет:

- `config_getall.json` — c = «rest /api/v3/capital/config/getall», t = 1795261750806 (2026-11-21 11:49:10.806 UTC),
  источник raw/mexc/1795260783987.jsonl.zst; оригинал 5199045 байт, sha256
  15b7a44a41eaf553a43963bb8ee3f43b18fe839582e57b2e2ad1819817acfbde (9651 монета; сеть Arbitrum One: network
  «Arbitrum One(ARB)», netWork «ARB» — 206 монет; BSC: network «BNB Smart Chain(BEP20)», netWork «BSC» — 2731).
  Отбор (8 монет, порядок ответа): USDT; первые четыре монеты с контрактом в ARB и BSC (RDNT, USDC, MLK, SKATE — у MLK и
  SKATE один адрес в обеих сетях); первые три с ARB без BSC (CATCHOLD, SEX, BFR).

Вырезка (этот репозиторий): `config_getall.json` — 21264 байт, sha256
055c5a680f4fced3f2ec9e6f9d812c8ecddf341fd1243a75c77275599d0d9230 (JSON с отступом 1, объекты монет — без изменений).

exchangeInfo (t = 1795261715798) тесту не нужен — не положен.

Кто читает: tests/core_net_chain_489_mexc.rs.
