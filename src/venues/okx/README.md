# venues/okx — адаптер OKX (спот, API v5)

Реализует контракт `gateway/contract.rs` (решение 161). Только спот: `instType=SPOT`, ордер `tdMode=cash`, режим счёта
Spot (`acctLv = 1`). Всё — только по официальной документации OKX; разбор перед подключением —
`docs/notes/okx-analysis-2026-11-05.md`, как закрыт каждый из 30 уроков — `docs/notes/okx-integration-2026-11-05.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://www.okx.com/docs-v5/en/ | Документация v5: подпись, лимиты, эндпоинты, WebSocket, коды ошибок (одна страница; разделы названы по оглавлению) |
| https://www.okx.com/docs-v5/log_en/ | Журнал изменений: домен openapi.okx.com, закрытие порта 8443, checksum стакана = 0 |
| https://github.com/okxapi/python-okx (v0.4.4) | Официальный SDK: `okx/utils.py` (подпись, время), `okx/websocket/WsUtils.py` (вход WebSocket) |
| https://www.okx.com/en-us/help/trading-fee-rules-faq | Скидки за OKB нет |

Из среды разработки `www.okx.com` закрыт прокси — документация прочитана пересказчиком, живых ответов нет. **Каждое
поле сверить на живых записях** (команды — `tests/README.md`, раздел OKX).

## Домен API — настройкой

Домен зависит от сущности счёта («Overview → Production Trading Services»): основной `https://openapi.okx.com`
(WebSocket `wss://ws.okx.com/ws/v5/{public,private,business}`), EEA — `eea.okx.com`, US — `us.okx.com`. Адаптер домен
не выбирает: `venue.rest_url`, `venue.ws_url` (public), `venue.ws_business_url` (business) задаёт Тигран.

## Счета OKX

- **Trading (18)** — ордера и баланс бота (`GET /api/v5/account/balance`).
- **Funding (6)** — вывод только отсюда («Only supported withdrawal of assets from funding account»).
- Депозит зачисляется на счёт адреса (`to` у `GET /asset/deposit-address`): адрес с `to = 18` берётся первым. Адрес
  только с `to = 6` — при `venue.deposit_to_spot = да` (после зачисления перевод 6 → 18), иначе отказ с причиной.
- Перевод — `POST /api/v5/asset/transfer` (type 0) и проверка `GET /asset/transfer-state` до success / failed
  (`venue.transfer_poll_ms`, `venue.transfer_wait_ms`): ответ на перевод не значит «переведено».
- Перед выводом при `venue.withdraw_from_trade = да` — перевод 18 → 6.

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки площадки, секреты (ключ, секрет, passphrase), HTTP, журнал, сырые сообщения |
| `sign.rs` | Подпись Base64(HMAC-SHA256(timestamp+METHOD+путь?запрос+тело)), время ISO 8601 с мс, вход WebSocket |
| `rest.rs` | REST: публичные и подписанные GET/POST, строка запроса = строка подписи; резерв только при сбое сети |
| `reply.rs` | Разбор {code, msg, data} и sCode/sMsg; 50011/50013/50040/50061/58102, HTTP 429 — пауза пути (`venue.rate_limit_pause_ms`) |
| `errors.rs` | Коды OKX → единые коды (501xx ключ и время, 51008 баланс, 58207 адрес вне списка, 58237 rcvrInfo …) |
| `num.rs` | Числа и поля без потери точности («» — нет значения) |
| `symbols.rs`, `symbols_parse.rs` | Справочник `GET /api/v5/public/instruments?instType=SPOT`: tickSz, lotSz, minSz, maxLmtSz, state, ruleType |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление, подписка; соединений ≤ 3/с, ≤ 30 на канал |
| `fees.rs` | Ставки `GET /api/v5/account/trade-fee` по паре фоном (≤ 2/с; ставка = −taker — у OKX комиссия отрицательна) |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети `GET /api/v5/asset/currencies` (с ключом; весь — вне рабочих потоков; одна — `ccy=`), chain OKX по сети ядра, комиссия вывода |
| `deposit_addr.rs` | Адрес депозита (кэш; Trading раньше Funding; tag/memo — не поддерживается) |
| `deposits.rs` | Статус депозита по txId; удержания вывода — депозиты в state 1 «deposit credited» (решение 150) |
| `transfers.rs` | Вывод `POST /asset/withdrawal` (dest 4, clientId, amt = сумма − fee), статус и поиск по clientId |
| `fund_move.rs` | Перевод Funding ↔ Trading с проверкой transfer-state |
| `account.rs` | Балансы Trading (все и одной монеты), «Проверить подключение» (`/account/config`: perm, acctLv, uid = mainUid, feeType, IP) |
| `all_balances.rs` | Панель «Балансы» (решение 144): Trading, Funding, Simple Earn, итоговая оценка — параллельно, только чтение |
| `order_rules.rs` | Правила ордера — одни у ордера и `check_order` (решение 134); пробного ордера для спота нет |
| `trade.rs`, `order_state.rs` | Ордер limit-цены типа ioc/fok со своим clOrdId, итог по ordId (accFillSz, avgPx, fee, feeCcy), отмена остатка, поиск по clOrdId |
| `depth.rs` | Разбор кадров WS: pong, event (subscribe/login/error/notice), books5, books, данные каналов |
| `local_book.rs` | Локальный стакан канала books: снимок + изменения, prevSeqId = прежний seqId, пустой кадр «живо» |
| `ws.rs`, `ws_conn.rs`, `ws_req.rs`, `book.rs` | Соединения стаканов: подписка запросами ≤ 64 КБ, «ping», разрыв → пересподписка пары, стаканы в памяти и котировка |
| `user_stream.rs`, `user_stream_conn.rs`, `deposit_event.rs` | Поток аккаунта на business: вход, deposit-info → MarketEvent::Balance (депозит с txId) |

## Что важно знать

- Пинг — строка `ping` после `venue.ws_ping_ms` без кадров (< 30 000 мс: сервер рвёт молчащее дольше 30 с); тишина
  дольше `venue.ws_silence_ms` — обрыв. Предел 480 запросов subscribe/unsubscribe/login в час на соединение — при
  исчерпании переподключение. Числа каналов на соединение в документации нет — `venue.ws_symbols_per_conn` по живому
  отказу (коды закрытия 4008/4009).
- `checksum` стакана устарел («fixed to 0») — целостность по prevSeqId/seqId.
- Минимальной суммы ордера в справочнике нет — отказ биржи 51020; maxLmtAmt — в USD, правилами не сверяется.
- Скидки за OKB нет (урок 23 неприменим); «Проверить подключение» требует feeType = 0 (комиссия в получаемой монете).
- Вывод — только на проверенные адреса из кабинета OKX (58207); `rcvrInfo` (Travel Rule) не отправляется — 58237 =
  отказ с причиной (вопрос Тиграну).
- Каналы торгового счёта `account` / `balance_and_position` причину «депозит» не сообщают — не используются; зачисления —
  deposit-info. Итог ордера — опросом REST (канал `orders` «Data will not be pushed when first subscribed»).
- Запись окна (решение 125) для OKX не сделана — `window_tap` отвечает NotApplicable (по умолчанию контракта).
- Функции контракта только для DEX отвечают `NotApplicable`.
