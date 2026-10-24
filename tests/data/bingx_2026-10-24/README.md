# tests/data/bingx_2026-10-24 — живые ответы REST BingX 2026-10-24

Куски ответов из сырого журнала адаптера (`data/raw/bingx`, записи «rest <путь>»), без изменений внутри объектов —
отобраны только нужные тесту монеты (обёртка ответа `code`, `msg`, `timestamp` — как прислал BingX):

- `symbols.json` — `GET /openApi/spot/v1/common/symbols`, ответ 2026-10-24 19:02:18 UTC (последний в журнале запуска).
  Пары: девять status 1 с `apiStateBuy`/`apiStateSell` = false и сетью BEP20 (B, NIULAI, RWA, SKYAI, TA, TAG, TUT, USD1,
  XPIN), status 10 (SEA, RAMP), status 25 (SOLV), status 0 (ZBC), обычные status 1 (BNB, CAKE, EGLD, BLUAI, XEC, PONS).
- `config_getall.json` — `GET /openApi/wallets/v1/capital/config/getall`, ответ 2026-10-24 19:51:29 UTC: те же монеты
  и USDT со всеми сетями. BEP20 без адреса контракта: BNB (`contractAddress: ""`), ZBC и RAMP (поля нет);
  у PONS сети BEP20 нет; у BLUAI в BEP20 закрыт ввод, у XEC — вывод.

Наблюдения по всему ответу symbols того же запуска: 2293 пары, status 0 — 1589, 1 — 650, 10 — 43, 25 — 11;
`apiStateBuy` и `apiStateSell` — boolean у всех пар и у всех совпадают (пар «можно только купить» или «только
продать» нет). Пар status 1 с закрытым API — 36, из них с сетью BEP20 — девять из списка выше.

Кто читает: tests/bingx_pairs_filter.rs.
