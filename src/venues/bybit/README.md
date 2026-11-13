# venues/bybit — адаптер Bybit (API V5, только спот)

Реализует контракт `gateway/contract.rs` (решение 136). Только `category=spot`: фьючерсов, маржи и опционов нет
(ордер — `isLeverage=0`; права деривативов у ключа — только предупреждение в «Проверить подключение», решение 243). Чек-лист уроков BingX/MEXC и
как каждый закрыт — `docs/notes/bybit-integration-2026-11-04.md`.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://bybit-exchange.github.io/docs/v5/intro | Документация V5 (исходник — github.com/bybit-exchange/docs, коммит 76aa20a от 2026-11-03) |
| https://bybit-exchange.github.io/docs/v5/guide | Подпись (X-BAPI-*), ответ {retCode, retMsg, result}, адреса REST |
| https://bybit-exchange.github.io/docs/v5/rate-limit | 403 «access too frequent» (10 минут), 10006, заголовки X-Bapi-Limit*, лимиты эндпоинтов |
| https://bybit-exchange.github.io/docs/v5/error | Коды ошибок (UTA, Spot Trade, Asset) |
| https://bybit-exchange.github.io/docs/v5/ws/connect | WebSocket: адреса, auth, ping, пределы args (10 на запрос, 100 на соединение, 21000 символов) |
| https://bybit-exchange.github.io/docs/v5/acct-mode | Режимы аккаунта: classic недоступен, UTA 1.0 / 2.0 |
| github.com/bybit-exchange/pybit | Официальный SDK — сверка обработки snapshot/delta (непрерывность `u` SDK не проверяет) |

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты, HTTP, журнал, сырые сообщения, окна, состояние приватного потока; символ `CAKEUSDT` |
| `delist.rs` | Решение 226: делистинги — `stTag` = "1" («special treatment label») спот-пар GET /v5/market/instruments-info с venue.quote_asset — запись с признаком st (блок только при delist.st_block = да) + анонсы (delist_ann.rs) |
| `delist_ann.rs` | Решение 226 (уточнение 08.11): анонсы GET /v5/announcements/index?locale=en-US&type=delistings БЕЗ tag (спот с сентября 2026 — без тега Spot): в заголовке «delist», без тегов Derivatives / WEB3 и заголовков про фьючерсы / залог; монеты — из «Delisting of A, B», иначе со страницы url («Delisted trading pairs: HFTUSDT»); время — из title/description, иначе со страницы («… will end after Oct 28, 2026, 8:00AM UTC») |
| `http.rs` | HTTP-обмен со сроками gateway/http.rs и заголовками X-Bapi-Limit* |
| `rest.rs` | Подпись HMAC SHA256 (`timestamp+key+recv_window+query|body`), GET/POST, резервный адрес только при сбое сети |
| `resolve.rs` | Решение 231: исход операции «неизвестен» по «чем найти» из ошибки — ордер по своему ID / ID ордера (те же запрос, отмена и доведение, что у постановки); вывод (дополнение 08.11) — тем же совпадением «монета + сеть + адрес + сумма с момента отправки», что `transfers.rs` сразу после сбоя: ровно одна запись — создан, ноль — ядро спросит ещё, больше одной — человек |
| `reply.rs` | Разбор ответа; пауза 10 мин после HTTP 403; пауза эндпоинта до X-Bapi-Limit-Reset-Timestamp (Status=0, 10006) |
| `errors.rs` | retCode / HTTP → единые коды; «итог неизвестен» у ордера, вывода, перевода. Отказы вывода (решение 253, дополнение t82): 131083/131093/131066/131098 → InvalidAddress; 131065/131084/131086…131089/131094 → AccountRestricted; дополнение t85: 131095 (24 ч) → WithdrawLimit, у вывода 131004/131090…131092 → AccountRestricted; решение 263: 170035 «Submitted to the system for processing!» у ордера → «итог неизвестен» (поиск по orderLinkId) |
| `errors_fail.rs` | Сбои HTTP-обмена (`from_fail`, `fail_for`) — вынесены из `errors.rs` без изменения логики (решение 263); снаружи — `errors::from_fail` / `errors::fail_for` |
| `num.rs` | Числа из строк и чисел JSON без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Спот-пары instruments-info: basePrecision, tickSize, minOrderAmt, maxLimitOrderQty, status |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs` | Снимок пар (SpotPairs), фоновое обновление, подписка (как MEXC) |
| `fees.rs` | Ставки taker/maker по паре (`/v5/account/fee-rate`, 5/s) фоном |
| `market.rs`, `ws.rs`, `ws_conn.rs` | WebSocket стаканов `orderbook.{N}.{SYM}`: пачки ≤10 args, ≤100 на соединение, ping, переподключение; решение 202 — учёт свежести для сторожа ядра (gateway/feed_reg.rs), переподписка одной пары (`unsubscribe` + `subscribe`) и переподключение одного соединения по команде сторожа |
| `depth.rs`, `local_book.rs` | Разбор кадров (serde со ссылками), локальный стакан snapshot+delta с проверкой `u`, пересинхронизация |
| `book.rs` | Стаканы в памяти, котировка на объём, свежесть соединения и пары; решение 213 — срок подтверждения тихой пары (`book_confirmed`) |
| `book_print.rs` | Решение 213: `GET /v5/market/orderbook` (limit = `venue.depth_levels`) — первые N уровней ответа и стакана в памяти (по содержимому) и `u` ответа (по номеру, только при глубине 1000) для подтверждения тихой пары ядром; Решение 238: тот же запрос — снимок стакана для ордера (`snapshot`, контракт `quote_rest`), все уровни ответа |
| `wallet.rs`, `networks_parse.rs` | Монеты и сети (`/v5/asset/coin/query-info`), запрос одной монеты, стоимость вывода |
| `deposit_addr.rs` | Адрес депозита (master), tag — «не поддерживается» |
| `deposits.rs` | Статус депозита по txID |
| `transfers.rs` | Вывод (forceChain=1, feeType=1, requestId, accountType = venue.withdraw_account), статус, поиск при неизвестном итоге |
| `fund_move.rs` | FUND → UNIFIED после зачисления (venue.deposit_to_spot) |
| `withdrawable.rs` | Решение 166: удержание вывода — `GET /v5/asset/withdraw/withdrawable-amount` (availableBalance − withdrawableAmount по счетам venue.withdraw_account); 131085 у вывода («delayed withdrawal») → WithdrawLocked (errors.rs) |
| `account.rs` | Балансы UNIFIED, баланс одной монеты, «Проверить подключение» (права, master, UTA, IP) |
| `all_balances.rs`, `all_balances_parse.rs` | Решение 144: панель «Балансы» — UNIFIED (wallet-balance), FUND (query-account-coins-balance), прочие счета (asset-overview: Earn, боты, копитрейдинг, займы…), параллельно, только чтение |
| `order_rules.rs` | Правила ордера — одни у ордера и check_order; предел цены price-limit (venue.price_limit_check) |
| `trade.rs`, `order_state.rs`, `fills.rs` | Limit + IOC/FOK, ожидание итога (поток или REST), отмена по сроку, комиссия и её монета |
| `market_sell.rs` | Решение 268: рыночная продажа (контракт `trade_market_sell`) — ордер типа «market» на количество базовой монеты после отказа лимитного ордера «цена вне диапазона»; правила пары и разбор итога — те же, что у лимитного (`order_rules.rs`, `trade.rs`); код PriceOutOfRange рыночный ордер не отдаёт |
| `user_stream.rs`, `user_stream_conn.rs`, `private_cache.rs` | Приватный поток order.spot, execution.spot, wallet (venue.private_stream) |
| `tap.rs`, `feed.rs` | Окна (решение 125): кадры стакана, publicTrade, стакан окна `orderbook.{venue.window_depth_limit}` |

## Что важно знать

- Счёт: торговля и балансы — UNIFIED. Депозит по умолчанию приходит на FUND; решение Тиграна (04.11): в кабинете Bybit
  назначить зачисление депозита на UNIFIED (Set Deposit Account), `venue.deposit_to_spot` не задавать — бот перевод
  FUND → UNIFIED не делает. Запасной вариант остаётся: `venue.deposit_to_spot = да` → `deposit_to_trading` переводит
  FUND → UNIFIED (тогда нужно право Wallet: AccountTransfer). Прочитать счёт зачисления API не даёт (есть только запись),
  «Проверить подключение» его не проверяет.
- «Проверить подключение» пишет тип счёта явно: UTA 1.0 / UTA 1.0 pro / UTA 2.0 / UTA 2.0 pro (unifiedMarginStatus 3–6);
  classic (1) и прочее — отказ.
- Пробного ордера у спота нет (`/v5/order/pre-check` — только inverse/linear/option): `check_order` — только правила.
- Признака «торгуется через API» у пары нет: покупка/продажа через API = статус `Trading`.
- Процентная комиссия вывода (`withdrawPercentageFee` ≠ 0) ядром не учитывается — комиссия вывода такой сети «неизвестна».
- Изменения баланса ядру (решение 76) не шлются: wallet описывает только UNIFIED и без причины; депозит — опросом статуса.
- Непрерывность `u` у спота в документации не обещана; на живой записи 04.11 `u` шёл строго +1. Строгая проверка —
  `venue.book_seq_strict` (да — пропуск номера = разрыв → пересинхронизация; не задана — только рост `u`).
- Сторож пар (решения 202/213, 08.11): `feed_health` — scheduled = false, per_pair = true, **confirm = true**: по номеру
  `u` — только при `venue.depth_levels` = 1000 (Get Orderbook: «For spot, corresponds to u in the 1000-level WebSocket
  orderbook stream»; ветка main исходника документации пишет «200-level» — расхождение в docs/TASKS.md); иначе — по
  содержимому (первые N уровней `GET /v5/market/orderbook` против стакана в памяти). `seq` не используется. Лимит
  REST — 600 запросов за 5 с на IP на все запросы.
