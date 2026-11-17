# t165 — адрес депозита создаётся ботом (решение 379), 17.11.2026

Ветка `t165/deposit-addr-create` от `c2d6aba2`. Локально не собиралось (сборка, clippy и тесты — в GitHub).

## Почему

Живой бот 17.11: направление ESIM bsc_dex → poloniex давало сигналы с прибылью ~1 USDT (спред 20–46 %), предпроверка
94 раза подряд отказала «poloniex: нет адреса депозита ESIM в сети BSC — … создайте адрес в кабинете Poloniex
(Deposit)»; BANK → poloniex — 6 раз. Сделки потеряны. Решение Тиграна (17.11): «на всех биржах этот адрес должен
автоматом создаваться» — где у биржи есть официальный метод создания, бот создаёт адрес сам.

## Как сделано

- Выбор «создать / не создавать» — `gateway/addr_claim.rs`: `on_missing()` (чистая логика `decide(allowed)`, тесты).
  Предпроверка, перевод сделки, ручные команды — создают; фоновый прогрев адресов без `venue.deposit_warm_create = да` —
  не создаёт (NotApplicable), как у KuCoin (решение 316). Новых настроек нет.
- После создания — журнал `venue.deposit_address` (Info) без самого адреса — как у KuCoin (решение 208).
- Отказ биржи в создании — её код и сообщение дословно, с пояснением «… отказал в создании адреса …», без «создайте в
  кабинете».
- Ядро (`core/precheck_deposit.rs`): общий хвост «(создайте адрес в кабинете биржи)» у любого отказа адреса убран —
  что делать, пишет адаптер.

## Таблица 14 бирж

| Биржа | Что было (код адаптера) | Документация (ссылка, цитата) | Что сделано |
|---|---|---|---|
| Poloniex | Нет адреса в GET /wallets/addresses → InvalidAddress «создайте адрес в кабинете Poloniex (Deposit)» | https://api-docs.poloniex.com/spot/api/private/wallet — «New Currency Address» POST /wallets/address: «Create a new address for a currency», currency — «the currency to use for the deposit address», ответ address — «the newly created address». Сеть, лимит, право ключа, повторное создание — не описаны | Создаёт (`venues/poloniex/deposit_addr_new.rs`): то же имя монеты сети (coin из /v2/currencies), что в GET; сеть PAYMENT_ID — отказ, как раньше; адрес проверяется той же `pick`, что ответ GET; кэш |
| MEXC | Нет записи сети в GET /api/v3/capital/deposit/address → «создайте его в кабинете MEXC (Депозит)» | https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/generate-deposit-address-supporting-network — «Generate deposit address (supporting network)» POST /api/v3/capital/deposit/address, «SPOT_WITHDRAW_WRITE», вес 1, coin и network обязательны; ответ address, coin, memo, network (пример — массив). Какое имя сети ждёт network — не сказано | Создаёт (`venues/mexc/deposit_addr_new.rs`): network — то же отображаемое имя, что в GET; запись сети выбирается той же `pick`; memo — отказ, как раньше; кэш |
| KuCoin | Создаёт (решение 208, Add Deposit Address V3) | https://www.kucoin.com/docs-new/api-3470140: «If the returned data is empty, you may need to add the deposit address first» | Без изменений |
| Gate | Нет сети в multichain_addresses → «создайте его в кабинете Gate (Депозит)» | Официальный SDK gateapi-python, docs/WalletApi.md: GET /wallet/deposit_address — «Generate currency deposit address», ответ 200 «Address successfully generated» (сам запрос создаёт адрес) | Текст «создайте в кабинете» убран: «Gate не вернул адрес … в multichain_addresses … этой сети нет». Поведение прежнее (отказ) |
| Kraken | Claim (PUT /funding/v1/deposit/address) только при `venue.deposit_address_claim` = да, иначе «создайте в кабинете … или включите …» | https://docs.kraken.com/exchange/guides/rest/funding.md — claim есть; «limited (typically limit: 5)» | Не тронуто (существующая настройка). Вопрос владельцу: сделать claim автоматическим |
| BingX | Нет адреса сети → «нет адреса депозита в сети …» | api-ai-skills (официальный GitHub BingX), spot-wallet: только Query Main Account Deposit Address; создание — «Create Deposit Address for Sub-account» POST /openApi/wallets/v1/capital/deposit/createSubAddress — только для субаккаунта | Документация не определяет (для основного аккаунта метода нет). Поведение прежнее, вопрос в TASKS |
| Binance | Нет address → «создайте его в кабинете (Кошелёк → Депозит)» | https://developers.binance.com/docs/wallet/capital/deposite-address — «Fetch deposit address with network»; про создание не сказано; рядом — только список адресов и «One click arrival» | Документация не определяет. Поведение прежнее, вопрос в TASKS |
| Bitget | Нет address → «проверьте в кабинете Bitget (Deposit)» | https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account — Get Deposit Address (10/1s); метода создания нет; про создание GET не сказано | Документация не определяет. Поведение прежнее, вопрос в TASKS |
| Bybit | Нет записи сети → «создайте его в кабинете Bybit (Deposit)» | Репозиторий документации bybit-exchange (docs/v5/asset/deposit, копия от 06.11.2026): Get Master Deposit Address — «Query the deposit address information of MASTER account»; метода создания нет. Страницу сайта прочитать не удалось (доступ не дан) | Документация не определяет. Поведение прежнее, вопрос в TASKS |
| CoinEx | Нет address → «проверьте в кабинете CoinEx (Deposit)» | https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/get-deposit-address — про создание не сказано; «Update Deposit Address» POST /assets/renewal-deposit-address (ответ address, memo) — создаёт ли адрес, если его не было, не сказано | Документация не определяет. Поведение прежнее, вопрос в TASKS |
| HTX | Нет адреса сети → «проверьте в кабинете HTX (Deposit)» | https://huobiapi.github.io/docs/spot/v1/en/ (вся страница) — только Query Deposit Address и Query Deposit Address of Sub User | Документация не определяет. Поведение прежнее, вопрос в TASKS |
| OKX | Нет адреса сети → «создайте его в кабинете OKX (Deposit)» | Страницу https://www.okx.com/docs-v5/en/ в задаче прочитать не удалось (доступ не дан). Не документация: в копии SDK python-okx 0.4.4 — только GET /api/v5/asset/deposit-address | Не установлено. Поведение прежнее; проверить docs-v5 (TASKS) |
| Phemex | Нет address → «проверьте в кабинете Phemex (Deposit)» | https://phemex-docs.github.io/ (копия страницы) — только «Query deposit address by currency» и «Query deposit address information» | Документация не определяет. Поведение прежнее, вопрос в TASKS |
| XT | DEPOSIT_006/007 → «XT его создаёт, повторите позже»; пустой address → «создайте его в кабинете» | https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetDepositAddress; errorCode.md: DEPOSIT_006 «No deposit address, please try it later», DEPOSIT_007 «Address is being generated, please try it later»; метода создания нет | Документация прямо не определяет (только коды). Поведение прежнее, вопрос в TASKS |

## Итог старта у сигнала (п. 6 задачи)

- `core/start_note.rs`: текст отказа на входе (`refused`), итог старта (`note`: «сделка запущена» / сухой режим / отказ)
  и ключ `start|направление`.
- Запись `core.candidate` (без эпизодов — card_job.rs; с эпизодами — EpMsg::Record → episode_worker.rs → Open::note →
  episode_notify.rs) всегда содержит строку «Итог старта: …».
- `core.start_refused` — через `last_reject.note` (reject_log.rs), как `core.precheck`: при `journal.repeat_flush_ms`
  повтор — счётчик и время последнего в той же строке; без неё — как у core.precheck (новая строка на другой текст).
  Started — строка закрывается итоговым счётчиком. Поле `Inner.start_refused` удалено.

## Без теста на записи

- POST /wallets/address (Poloniex) и POST /api/v3/capital/deposit/address (MEXC) — тесты разбора на примерах из
  документации (`tests/data/poloniex_doc_2026-11-17`, `tests/data/mexc_doc_2026-11-17`). Живой ответ — в сыром журнале
  сервера (`<data-dir>/raw/poloniex/*.jsonl.zst`, `<data-dir>/raw/mexc/*.jsonl.zst`, строка «rest /wallets/address» /
  «…/capital/deposit/address») при первом создании — записать в tests/data.
- Имя сети MEXC в POST (отображаемое, как в GET) — не из документации, проверить первым созданием.
