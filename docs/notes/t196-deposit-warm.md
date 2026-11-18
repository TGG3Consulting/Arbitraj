# t196 — адрес депозита из памяти и прогрев: лимиты бирж (19.11.2026)

Ветка `t196/precheck-fast` от `7fefaeba`. Решение 411. Локально не собиралось (сборка, clippy и тесты — в GitHub).

## Зачем таблица

Прогрев адресов (решение 316: `venue.deposit_warm`, `venue.deposit_warm_gap_ms`, `venue.deposit_warm_create`) по
живому журналу 18.11 не задан ни у одной биржи — первая сделка по монете после перезапуска ждёт запрос адреса
(ESIM → kucoin 284 мс, LAB → bitget 262 мс). Чтобы включить его, нужна пауза между запросами, которая не упирается в
лимит биржи и оставляет квоту запросам адреса по триггерам (решение 411).

## Как считана пауза

- **Минимум по документации** = окно ÷ (лимит ÷ вес запроса) — чаще этого прогрев один упрётся в лимит.
- **Предложение** = минимум × 10, округлённо вверх: прогрев берёт не больше ~10 % квоты эндпоинта, остальное — сделкам
  (запросы адреса по триггерам, решение 411), выводу и опросам на тех же пулах. Это совет для таблицы первого
  запуска, не значение движка (стартовых значений нет). Прогрев и так уступает бирже, занятой сделкой (решение 316).
- Где лимита в документации нет или единица не указана — числа нет, вопрос Тиграну.

## Таблица

| Биржа | Эндпоинт адреса (код адаптера) | Лимит по официальной документации (ссылка, цитата) | Минимум, мс | Предлагаемое `venue.deposit_warm_gap_ms` | Создаёт ли запрос адреса новый адрес (для `venue.deposit_warm_create`) |
|---|---|---|---|---|---|
| Binance | GET /sapi/v1/capital/deposit/address | https://developers.binance.com/docs/wallet/capital/deposite-address — «IP Weight 10»; https://developers.binance.com/docs/wallet/general-info — «Each endpoint with IP limits has an independent 12000 per minute limit» | 50 | 500 | Документация не говорит («Fetch deposit address with network»). Бот не создаёт |
| BingX | GET /openApi/wallets/v1/capital/deposit/address | Лимита эндпоинта в доступной документации не нашёл: страницы bingx-api.github.io — приложение без текста для чтения, api-ai-skills (spot-wallet) лимитов не пишет | — | нет числа — вопрос Тиграну | Документация не говорит; метод создания есть только для субаккаунта (решение 379). Бот не создаёт |
| Bitget | GET /api/v2/spot/wallet/deposit-address | https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account#get-deposit-address — «10 times/1s (User ID)» (шапка адаптера, docs/notes/bitget-analysis-2026-11-05.md) | 100 | 1000 | Документация не говорит, метода создания нет. Бот не создаёт |
| Bybit | GET /v5/asset/deposit/query-address | https://bybit-exchange.github.io/docs/v5/rate-limit — в таблице Asset: «300 req/min» (не повышается); общее правило страницы — «rolling time window per second and UID» | 200 | 2000 | Документация не говорит, метода создания нет. Бот не создаёт |
| CoinEx | GET /assets/deposit-address | https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/get-deposit-address — группа «Query spot accounts (10r/1s)» (шапка адаптера; группа общая с запросами счёта) | 100 | 1000 | Документация не говорит (есть «Update Deposit Address» — создаёт ли при отсутствии, не сказано). Бот не создаёт |
| Gate | GET /wallet/deposit_address | https://www.gate.com/docs/developers/apiv4/en/ — «Frequency limit rule», Wallet: отдельной строки у этого эндпоинта нет; строка «The Others: 200r/10s per endpoint» | 50 | 500 | **Да, сам GET** — SDK gateapi-python docs/WalletApi.md: «Generate currency deposit address», ответ 200 «Address successfully generated» (решение 379). Запрет создания прогрева (gateway/addr_claim.rs) этого не останавливает: создание — на стороне биржи по GET. Включение прогрева у Gate = адреса по всем монетам, где Gate — B |
| HTX | GET /v2/account/deposit/address | https://huobiapi.github.io/docs/spot/v1/en/#query-deposit-address — «20times/2s» (шапка адаптера) | 100 | 1000 | Документация не говорит (только Query Deposit Address). Бот не создаёт |
| Kraken | GET /funding/v2/deposit/addresses (+ метод сети) | https://docs.kraken.com/exchange/guides/rest/ratelimits.md — счётчик приватных REST: Starter 15 / −0,33 в с, Intermediate 20 / −0,5, Pro 20 / −1; «Ledger/trade history calls increase the counter by 2», прочие — 1. Эндпоинты /funding/ на странице не названы | ~3000 (Starter, если правило действует на /funding/) | 6000 (если правило действует на /funding/ — вопрос) | Список — нет; бот делает claim (PUT /funding/v1/deposit/address), когда годного адреса нет (решение 379, «Claiming can reserve a new address»). Прогрев без `venue.deposit_warm_create = да` claim не делает |
| KuCoin | GET /api/v3/deposit-addresses | https://www.kucoin.com/docs-new/api-3470140 — пул Management, вес 5 (шапка адаптера); https://www.kucoin.com/docs-new/rate-limit — VIP0 Management «2000/30s» (пул общий с выводом и переводами) | 75 | 750 | GET — нет («If the returned data is empty, you may need to add the deposit address first»); бот создаёт POST /api/v3/deposit-address/create при пустом ответе (решение 208). Прогрев без `venue.deposit_warm_create = да` не создаёт |
| MEXC | GET /api/v3/capital/deposit/address | https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/deposit-address-supporting-network — «Weight(IP): 10»; https://www.mexc.com/api-docs/spot-v3/general-info — «Each endpoint with IP limits has an independent 300 every 10 second limit» | 334 | 3400 (у MEXC пауза после 429 — на весь REST, решение 316) | Документация не говорит; создание — отдельный «Generate deposit address» (POST). Бот не создаёт (решение «379 (MEXC)») |
| OKX | GET /api/v5/asset/deposit-address | https://www.okx.com/docs-v5/en/ — Get deposit address: 6 запросов в 1 с, User ID (шапка адаптера) | 167 | 1700 | Не установлено (страницу docs-v5 при t165 прочитать не удалось). Бот не создаёт |
| Phemex | GET /phemex-deposit/wallets/api/depositAddress | https://phemex-docs.github.io/ — «Group capacity»: Others «100/minute»; «Others APIs which is not in contract or spotOrder group is categorized into other group» (эндпоинта нет в группах Contract и SpotOrder) | 600 | 6000 (группа Others — общая для всех прочих запросов) | Документация не говорит. Бот не создаёт |
| Poloniex | GET /wallets/addresses | https://api-docs.poloniex.com/spot/api/ — Rate Limits, Wallets: «Deposit Addresses» 50 / 75 / 100 / 150 / 200 по уровням VIP0 / 1–2 / 3–4 / 5–6 / 7–9; «Interfaces related to transactions and accounts are frequency limited based on the UID dimension». Единица в этой таблице не указана («per second» — только у публичной таблицы) | — | нет числа — единица не указана, вопрос Тиграну | GET — нет; бот создаёт POST /wallets/address, когда адреса нет (решение 379). Прогрев без `venue.deposit_warm_create = да` не создаёт |
| XT | GET /v4/deposit/address | https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetDepositAddress — лимит не указан; общий раздел (https://xt-com.github.io/xt4-api/) — лимиты пишутся у каждого интерфейса («N/s/ip», «N/s/apiKey»), у /v4/deposit/* их нет | — | нет числа — вопрос Тиграну | Документация не говорит; коды DEPOSIT_006 «No deposit address, please try it later», DEPOSIT_007 «Address is being generated» — адрес создаёт сама XT. Бот не создаёт |

## Как прогрев сочетается с решением 411 (переделка «по триггеру», Тигран 19.11)

- Прогрев кладёт адреса в ту же память адаптера под отпечатком ключа API (gateway/addr_book.rs). Предпроверка и вывод
  берут адрес из памяти без запроса к бирже. Запрос — только по триггеру (core/deposit_proof.rs): адреса нет в памяти
  под текущим ключом; смена секретов; вывод отклонён с кодом InvalidAddress; перевод дольше transfer.max_time_s или
  статус депозита DepositNotCredited. Ответ, отличный от памяти, — событие venue.deposit_address_changed.
- Смена секретов: прогрев забывает и полученные монеты (`kick_account`), проходит заново; монета, адрес которой под
  новым ключом уже в памяти, запроса не делает; под новым ключом память пустая — запросы с паузой
  `venue.deposit_warm_gap_ms`.
- Нагрузка: запросов адреса не больше, чем до решения 411 (адрес в памяти — запроса нет), плюс запросы по триггерам.

## Приватные потоки об адресе депозита (п. 2д переделки)

Есть ли у биржи по официальной документации приватный WebSocket или событие об изменении / создании адреса
депозита. Если бы было и адаптер держал поток — это был бы ещё один триггер. Не найдено ни у одной биржи из
прочитанных; не прочитано — так и записано, ничего не придумано.

| Биржа | Что прочитано | Канал об адресе депозита |
|---|---|---|
| Binance | https://developers.binance.com/docs/binance-spot-api-docs/user-data-stream — события `outboundAccountPosition`, `balanceUpdate`, `executionReport`, `ListStatus`, `eventStreamTerminated`, `externalLockUpdate` | Нет |
| Bybit | https://bybit-exchange.github.io/docs/v5/websocket/private/wallet — приватные темы Position, Execution, Fast Execution, Order, Wallet, Greek, Dcp, Strategy | Нет |
| OKX | https://www.okx.com/docs-v5/en/ — Funding Account WebSocket: «Deposit info channel», «Withdrawal info channel»; адрес — только REST «Get deposit address» | Нет (канал deposit-info — о депозитах, не об адресе) |
| Gate | https://www.gate.com/docs/developers/apiv4/ws/en/ — spot.orders, spot.orders_v2, spot.usertrades, spot.usertrades_v2, spot.balances, spot.margin_balances, spot.funding_balances, spot.cross_balances, spot.cross_loan, spot.priceorders, торговые запросы | Нет |
| MEXC | https://www.mexc.com/api-docs/spot-v3/websocket-user-data-streams — Spot Account Update, Spot Account Deals, Spot Account Orders | Нет (в прочитанном) |
| Kraken | https://docs.kraken.com/exchange/guides/websockets/authentication.md — названы ownTrades, openOrders; полный список каналов v2 не прочитан | Не установлено |
| HTX | https://huobiapi.github.io/docs/spot/v1/en/ — страницу в задаче прочитать не удалось (доступ не дан) | Не установлено |
| CoinEx | https://docs.coinex.com/api/v2/ — страницу в задаче прочитать не удалось (доступ не дан) | Не установлено |
| Bitget | https://www.bitget.com/api-doc/spot/websocket/private/Account-Channel — страницу в задаче прочитать не удалось (доступ не дан) | Не установлено |
| BingX, KuCoin, Phemex, Poloniex, XT | в задаче не читались | Не установлено |

Итог: триггера «биржа сообщила о новом адресе» в коде нет. Где не установлено — проверить документацию (TASKS t196).
