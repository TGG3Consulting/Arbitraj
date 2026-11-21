# DODO: пулы, которые бот не читает, и KJUDGE_ERROR — 16.11.2026

- Узел: свой BSC (geth 1.8.2) на старом сервере arb-node, http://127.0.0.1:8545, только чтение. Все eth_call на одном блоке **H = 133995628** (eth_blockNumber один раз), время снятия 2026-11-16 20:58:45 UTC. Вызовов в raw_rpc.json: 3955 (метод, params, result/error, как есть).
- Селекторы сверены с keccak: все 11 совпали с заданными (getDODOPoolBidirection 0x794e5538 и др.).

## 1. Откуда пулы
- `GET /api/coins` нового бота доступен (24 168 монет). У монет USDC, CAKE, PORTO, BNB, ETH, DEXE, 1INCH пулов площадки bsc_dex видов dodo-dvm/dpp/dsp в API 342, и у **всех valid = true** (valid = false нет ни у одного). Поэтому «пулы, которые бот не читает» взял по фабрикам: dex.factory dodo-dvm/dpp/dsp, getDODOPoolBidirection(монета, X) для X = USDT и всех 16 адресов dex.intermediate_tokens; найдено 328 пулов; **300 из них есть в /api/coins, 28 — нет**, это и есть «не читает» (не факт, что бот отбросил их именно из-за ошибок: причину в API не нашёл).
- Монеты: USDC 0x8ac76a51…, CAKE 0x0e09fabb…, PORTO 0x49f2145d…, BNB(WBNB) 0xbb4cdb9c…, ETH 0x2170ed08…, DEXE 0x6e88056e…, 1INCH 0x11111111….

## 3. eth_getCode(0xe5e9b0cab984b58b7e7ae17707d633295d5a4c4d)
Длина кода 6210 байт. Строка `4b4a554447455f4552524f52` (KJUDGE_ERROR) в байткоде: **НЕТ**. (Значит ошибка в getFeeRate приходит не из текста самого контракта 0xe5e9, а из вызываемого им другого контракта — не факт.)

## 2. Итог по пулам (блок H)

| вид | пулов | из них не в API бота | KJUDGE_ERROR в getUserFeeRate(0x0) и getFeeRate(пул,0x0) | прочие revert |
|---|---|---|---|---|
| dodo-dvm | 168 | 3 | 2 (из них не в API: 2) | 1 |
| dodo-dpp | 102 | 21 | 15 (из них не в API: 15) | 6 |
| dodo-dsp | 58 | 4 | 4 (из них не в API: 4) | 0 |

Все 21 пул с KJUDGE_ERROR в getUserFeeRate(0x0) — среди 28 «не в API» (в API ни одного такого нет). У пулов в API ошибки только у querySellBase/Sell Quote с трейдером 0x0…0: `TARGET_IS_ZERO` (для 0x0-адреса это ожидаемо).

### 28 пулов, которых нет в /api/coins бота (все вызовы — pools_answers.json)

| пул | вид | монеты | base | quote | getPMMState | getUserFeeRate(0x0) | getFeeRate(пул,0x0) |
|---|---|---|---|---|---|---|---|
| 0xb6f4cb89916dd529a5a6650e98269662f06f6d7d | dodo-dvm | USDC | 0x55d398326f… | 0x8ac76a51cc… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0xdc075956abeefaf00df83a5007ec95455227c382 | dodo-dvm | BNB | 0x55d398326f… | 0xbb4cdb9cbd… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x0ab58740a864d4fffdeffa39c24bf2845cde9a09 | dodo-dvm | BNB | 0xb8b73763e4… | 0xd67e71b234… | OK | ERR execution reverted: 0x | OK |
| 0x298ba9cd3acd572dce608ec682501c899628f176 | dodo-dpp | USDC | 0x8ac76a51cc… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x90558b38a2d98023ac5310146c43e2ebb8159ad2 | dodo-dpp | USDC | 0x8ac76a51cc… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x9327c30961f35fff6ee0684d768b0d49fcae964f | dodo-dpp | USDC | 0x8ac76a51cc… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0xb4d986ac55e185d2d7f431d5ae97345e1d4cecef | dodo-dpp | USDC | 0x8ac76a51cc… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x707c10dc442f5e2f5f48fac9396653d0b6a1d648 | dodo-dpp | USDC | 0x8ac76a51cc… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x6fbc49e9dd32a9d93bb218cbfc459dd8b454a9b8 | dodo-dpp | USDC | 0x55d398326f… | 0x8ac76a51cc… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0xf0af9cdfe25de5b0559f3ee3399dd5160a38bb36 | dodo-dpp | USDC | 0x55d398326f… | 0x8ac76a51cc… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x39a5d6ce3ed543fc107209de644255516eea3174 | dodo-dpp | ETH,USDC | 0x2170ed0880… | 0x8ac76a51cc… | ERR execution reverted: ORACLE_NOT_FEASIBLE: 0 | OK | OK |
| 0xfe3871b9d8812673b7744e36b60ac653126c4b00 | dodo-dpp | CAKE,USDC | 0x8ac76a51cc… | 0x0e09fabb73… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x1e3a6de647d0d0761528e6743320100d1aed0e3b | dodo-dpp | CAKE,USDC | 0x0e09fabb73… | 0x8ac76a51cc… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x3ee0cc642d96189afc9066c4b0de3db89b038110 | dodo-dpp | BNB,CAKE | 0x0e09fabb73… | 0xbb4cdb9cbd… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x81f94ab7b5b8b2ae8570af87fc7ef30be4980d4e | dodo-dpp | CAKE | 0x0e09fabb73… | 0x7130d2a12b… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x88d34971a8ee35826ba856dc489a53b19c75499f | dodo-dpp | BNB | 0xbb4cdb9cbd… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0xfeafe253802b77456b4627f8c2306a9cebb5d681 | dodo-dpp | BNB | 0xbb4cdb9cbd… | 0x55d398326f… | ERR execution reverted: ORACLE_NOT_FEASIBLE: 0 | OK | OK |
| 0x5928e8f369a8905685843e2cbed4e88231e972ba | dodo-dpp | BNB | 0xbb4cdb9cbd… | 0x55d398326f… | ERR execution reverted: ORACLE_NOT_FEASIBLE: 0 | OK | OK |
| 0x05d968b7101701b6ad5a69d45323746e9a791eb5 | dodo-dpp | BNB | 0xbb4cdb9cbd… | 0x55d398326f… | ERR execution reverted: ORACLE_NOT_FEASIBLE: 0 | OK | OK |
| 0x3a60a76acae8feec74d6b5b665d4dbaab2abc406 | dodo-dpp | ETH | 0x2170ed0880… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x9ad32e3054268b849b84a8dbcc7c8f7c52e4e69a | dodo-dpp | ETH | 0x2170ed0880… | 0x55d398326f… | ERR execution reverted: ORACLE_NOT_FEASIBLE: 0 | OK | OK |
| 0x1dc7dcf8486e4511262c1abe67bc897b52e07e6d | dodo-dpp | ETH | 0x2170ed0880… | 0x55d398326f… | ERR execution reverted: ORACLE_NOT_FEASIBLE: 0 | OK | OK |
| 0x6b685b0ea1396bba46b131ec9042ed3abb8ba234 | dodo-dpp | DEXE | 0x6e88056e83… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x649bd388103890eab5da7881c66f016da581ae02 | dodo-dpp | 1INCH | 0xbb4cdb9cbd… | 0x1111111111… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x2b1aa9b392e6b377485c8efa14a87a4779d9524e | dodo-dsp | USDC | 0x55d398326f… | 0x8ac76a51cc… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0xc947c5deeaf13d6341d0a587292e1c7a2fd7d386 | dodo-dsp | PORTO | 0x49f2145d63… | 0x55d398326f… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x8cbd0c7fd3f032f4ff2ed1b36a7a5e792abe1e53 | dodo-dsp | PORTO | 0x55d398326f… | 0x49f2145d63… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |
| 0x72ceadbf93e8bdc98e77eda7c587257250421477 | dodo-dsp | BNB | 0x0efb5fd240… | 0xbb4cdb9cbd… | OK | ERR execution reverted: KJUDGE_ERROR: 0x08c379 | ERR execution reverted: KJUDGE_ERROR: 0x08c379 |

## 4. Журнал бота за сутки: что было за 30 с до каждого всплеска KJUDGE_ERROR

- Журнал: БД старого бота (до 16:36) + нового (после 16:45), все записи любого вида (время, вид, текст целиком) в `pre_burst_journal_30s.json`; все KJUDGE-записи — `kjudge_journal_entries_24h.json` (539 записей).
- Всплеск = запись dex.pool с KJUDGE_ERROR, перед которой за 60 с не было другой такой. Всплесков 20.

| начало всплеска UTC | записей за 30 с до | из них dex.pool | знакомое событие рядом (не факт, что причина) |
|---|---|---|---|
| 11-16T10:25:39 | 4905 | 4851 | деплой бота 10:16–10:17 |
| 11-16T11:22:00 | 47 | 4 | ? |
| 11-16T15:22:35 | 4017 | 4003 | перезапуск бота 15:14 |
| 11-16T17:02:24 | 4912 | 4867 | запуск нового бота ~16:45? |
| 11-16T17:17:40 | 74 | 8 | — |
| 11-16T17:18:47 | 62 | 8 | — |
| 11-16T17:21:05 | 69 | 3 | — |
| 11-16T17:27:07 | 207 | 1 | — |
| 11-16T17:28:18 | 173 | 5 | — |
| 11-16T17:29:49 | 223 | 5 | — |
| 11-16T17:31:06 | 165 | 4 | — |
| 11-16T17:32:40 | 187 | 3 | — |
| 11-16T17:33:50 | 152 | 5 | — |
| 11-16T17:49:39 | 823 | 688 | — |
| 11-16T17:55:54 | 210 | 1 | — |
| 11-16T17:57:39 | 126 | 5 | — |
| 11-16T18:00:31 | 152 | 5 | — |
| 11-16T20:26:02 | 238 | 10 | остановка/запуск старой ноды 20:25:11–20 |
| 11-16T20:41:39 | 272 | 12 | перезапуск новой ноды 20:41:05 |
| 11-16T20:52:12 | 233 | 8 | перезапуск новой ноды 20:51:38 |

Бо́льшая часть окон 10:25, 15:22, 17:02 и 17:49 — по 800–4900 записей dex.pool за 30 с (идёт загрузка пулов после запуска/перезапуска). Остальные окна — 47–272 записей, в основном core.signal_hold и core.candidate_rejected.