# dodo_kjudge_2026-11-16 — DODO «KJUDGE_ERROR»: журнал бота и сырые eth_call

Происхождение: сняты серверным ИИ 16.11.2026 по промпту docs/TASKS.md (t153 (а)), только чтение, бот не трогали.
Файлы положены как есть (sha256 совпадает с присланными); текст README сервера — `server_README.md` (как есть).

| Файл | Что | Откуда |
| --- | --- | --- |
| `journal_entries_24h.json` | 493 записи журнала dex.pool с KJUDGE_ERROR за 2026-11-15T20:51:42 – 2026-11-16T20:27:16 UTC (поля rowid, at, level, kind, venue, message, data) | SQLite журнала бота (обе БД: старый бот до 16:36, новый после 16:45) |
| `raw_rpc.json` | 55 вызовов JSON-RPC (метод, params, result/error): eth_blockNumber и eth_call на блоке H = 0x7fc93ec (133993452, 2026-11-16 20:42:26 UTC) | свой узел BSC (geth 1.8.2), HTTP 127.0.0.1:8545 старого сервера arb-node |
| `server_README.md` | README сервера: сводка и таблица | серверный ИИ |

## Что в raw_rpc.json (селекторы проверены keccak256)

- 4 вызова фабрик `getDODOPoolBidirection(address,address)` (0x794e5538): 0x790B4A80… (WBNB/USDT → DVM), 0xd9CAc3D9…
  (CAKE/USDT — 1 пул; ETH/USDT — 7 + 2 пула; DPP), 0x0fb98159… (WBNB/USDT, 5 + 1).
- На каждый из 5 пулов: version() 0x54fd4d50, _MT_FEE_RATE_MODEL_() 0xf6b06e70, _K_() 0xec2fd46d, _BASE_RESERVE_()
  0x7d721504, _QUOTE_RESERVE_() 0xbbf5ce78, getPMMStateForCall() 0xfd1ed7e9, getUserFeeRate(0x0) 0x44096609; у модели
  комиссии 0x18dfde99…: feeRateImpl() 0x39b96500 → 0xe5e9b0ca…; у него version() (откат без данных) и
  getFeeRate(pool, 0x0) 0x848cc303.

| Пул | Подпись (откуда) | version | K | B | Q | R | getUserFeeRate(0): lp / mt | getFeeRate(pool,0) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0xD5F05644… | dodo-dsp USDC (адрес из записи dex.pool_refresh, не из KJUDGE) | DSP 1.0.1 | 2e14 | 1.023e21 | 3.06e20 | 2 | 0 / 1e13 | 1e13 |
| 0x6098A563… | dodo-dpp BNB (то же) | DPP 1.0.0 | 3e17 | 3.05e20 | 2.44e23 | 1 | 5e15 / 0 | 0 |
| 0x426808dd… | dodo-dvm WBNB (фабрика) | DVM 1.0.2 | 1e18 | 1.515e11 | 9.77e13 | 1 | 3e15 / 7.5e14 | 7.5e14 |
| 0x9e6f399b… | dodo-dpp CAKE (фабрика, единственный DPP CAKE/USDT) | DPP 1.0.0 | 1e18 | 8.37e17 | 2.76e17 | 1 | 1.76e15 / 0 | 0 |
| 0x238b0b13… | dodo-dpp ETH (фабрика) | DPP 1.0.0 | 5e15 | 0 | 0 | 0 | 3e15 / 0 | 0 |

Поправка к таблице server_README.md: там столбцы getPMMStateForCall и getUserFeeRate(0) — обрезанное начало ответа,
а не значение; полные слова — здесь и в raw_rpc.json.

## Что из записей следует (факт записи) и чего в них нет

- Ни у одного из 5 пулов нет «K = 0 и нулевой резерв» — условия отката KJUDGE_ERROR по исходникам
  (DODOEX/contractV2 2f1bcda, FeeRateDIP3Impl.sol L116-L124); getUserFeeRate(0) ответил у всех. Значит, KJUDGE давали
  ДРУГИЕ пулы: в журнале адресов нет ни в одной записи.
- В журнале — одни и те же пулы по кругу: в каждом приёме пачки по одной записи Warn (admit.rs) и Error «за 6 попыток»
  (reload.rs) на пул; в приёме 20:26 — 21 пул (dodo-dpp USDC ×7, CAKE ×4, dodo-dsp PORTO ×2, остальные по 1).
  Фабрика 0xd9CA… знает один DPP CAKE/USDT, а в журнале 4 разных DPP CAKE — минимум 3 из них с другой парой.
- version() у развёрнутого feeRateImpl — откат без данных, а у FeeRateDIP3Impl 2f1bcda version() = «1.2.0»
  (L221-L223): код на BSC не совпадает с этим исходником — условие KJUDGE на BSC подтверждено только текстом ошибки.

Тест: tests/bsc_dex_dodo_kjudge_2026_11_16.rs. Разбор: docs/notes/t153-session-and-verify-2026-11-16.md, раздел 10.
