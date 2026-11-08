# venues/coinex — адаптер CoinEx (спот, API v2)

Реализует контракт `gateway/contract.rs` (решение 170). Всё — только по официальной документации CoinEx API v2; чек-лист
уроков (30 пунктов) и как каждый закрыт — `docs/notes/coinex-integration-2026-11-05.md`; анализ до кода —
`docs/notes/coinex-analysis-2026-11-05.md`. API v1 выключен 25.09.2024 (changelog) — только v2.

## Официальные источники

| Источник | Что там |
| --- | --- |
| https://docs.coinex.com/api/v2/guide | Адреса REST и WS, сжатие кадров WS, X-COINEX-TIMESTAMP / X-COINEX-WINDOWTIME, формат {code, data, message} |
| https://docs.coinex.com/api/v2/authorization | Подпись: METHOD + путь(с «/v2» и строкой запроса) + тело + время; HMAC-SHA256 → hex строчными; server.sign |
| https://docs.coinex.com/api/v2/error | Коды HTTP и WS; «не 200 — ответ шлюза, тело не разбирать» |
| https://docs.coinex.com/api/v2/rate-limit | Группы лимитов (30/с ордера, 50/с запрос ордера, 10/с счёт и история), 4213 / 3008 / 4001 |
| https://docs.coinex.com/api/v2/enum | market_status, order_type (ioc, fok), order_status, deposit_status, withdraw_status |
| https://docs.coinex.com/api/v2/spot/market/http/list-market | Пары: min_amount, base/quote_ccy_precision, status, is_api_trading_available |
| https://docs.coinex.com/api/v2/spot/market/ws/market-depth | depth.subscribe [market, limit 5/10/20/50, interval, if_full], depth.update |
| https://docs.coinex.com/api/v2/common/ws/ping | server.ping → "pong" |
| https://docs.coinex.com/api/v2/spot/order/http/put-order | Ордер (type ioc/fok, client_id ≤ 32 байт) |
| https://docs.coinex.com/api/v2/spot/order/http/get-order-status | Состояние ордера; «order without any transactions … will not be saved» |
| https://docs.coinex.com/api/v2/spot/deal/http/list-user-deals | Сделки пары (fee, fee_ccy, order_id) |
| https://docs.coinex.com/api/v2/account/fees/http/get-account-trade-fees | Ставка ключа — только по одной паре |
| https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-all-deposit-withdrawal-config | Монеты и сети: ввод/вывод, комиссия, минимумы, точность, memo |
| https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-assets-info | chain_info.identity — адрес контракта |
| https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/withdrawal | Вывод (адрес — в белом списке ключа) и история выводов |
| https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/get-deposit-address | Адрес депозита (address, memo) |
| https://docs.coinex.com/api/v2/assets/deposit-withdrawal/http/list-deposit-history | Депозиты по tx_id |
| https://docs.coinex.com/api/v2/assets/balance/http/get-spot-balance | Спот, а рядом — маржа, фьючерсы, финансовый счёт |

## Счета CoinEx

Торговля, депозит и вывод — со спот-счёта: funding-счёта у CoinEx нет (Transfer — только SPOT, MARGIN, FUTURES), поэтому
`deposit_to_trading` ничего не делает, а переводов между счетами в адаптере нет (проверить первым депозитом). Вывод — только
на адреса из белого списка API-ключа (кабинет → API), тогда без вторичной проверки (FAQ, код 3310).

## Файлы

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Реализация контракта `Venue` |
| `ctx.rs` | Окружение: настройки, секреты (access_id, secret_key), HTTP, журнал, сырые сообщения |
| `sign.rs` | Подпись REST и WS: HMAC-SHA256 → hex строчными, заголовки X-COINEX-* |
| `rest.rs` | REST: путь с «/v2» в адресе и подписи, параметры в порядке кода без кодирования, X-COINEX-WINDOWTIME, страницы |
| `reply.rs` | Разбор: HTTP не 200 — сырым; code 0; 4213 / 3008 / 4001 / HTTP 429 — пауза всех запросов |
| `errors.rs` | Коды CoinEx → единые коды (ключ, IP, права, баланс, лимит, отказ по операции, «итог неизвестен») |
| `num.rs` | Числа и флаги — строкой или числом / boolean без потери точности |
| `symbols.rs`, `symbols_parse.rs` | Пары: шаги 10^−precision, min_amount, status online + is_api_trading_available |
| `pairs.rs`, `pairs_watch.rs`, `subs.rs`, `market.rs` | Снимок пар и SpotPairs, фоновое обновление справочника, подписка (решение 131) |
| `fees.rs` | Ставки ключа по одной паре фоном (venue.fee_requests_per_sec); единицы — venue.fee_rate_percent |
| `wallet.rs`, `networks_parse.rs`, `contracts.rs` | Монеты и сети (весь — вне рабочих потоков, за Arc; одна — ?ccy=), адрес контракта из /assets/info |
| `delist.rs` | Решение 175: делистинги — `delisted_at` пар GET /v2/spot/market («Expected time to be delisted, in milliseconds»), только пары с venue.quote_asset |
| `deposit_addr.rs` | Адрес депозита (кэш; memo — не поддерживается) |
| `deposits.rs` | Статус депозита по tx_id |
| `transfers.rs` | Вывод on_chain со своей меткой remark, статус, поиск по remark |
| `account.rs` | Баланс спота, «Проверить подключение» |
| `all_balances.rs` | Панель «Балансы»: Спот, Маржа, Фьючерсы, Финансовый |
| `order_rules.rs` | Правила ордера — одни у ордера и у `check_order` (решение 134); пробного ордера нет |
| `trade.rs`, `order_state.rs`, `order_result.rs`, `order_fee.rs`, `deals.rs` | Ордер ioc/fok, опрос итога, отмена остатка, «не сохранён» → сделки, комиссия и её монета |
| `inflate.rs`, `depth.rs`, `ws.rs`, `ws_conn.rs`, `book.rs` | WS: распаковка кадров (буфер соединения), снимки if_full, server.ping, котировка |

## Чего нет и почему (вопросы Тиграну)

- **Поток аккаунта (решение 76)** — `balance.update` приходит только с новыми available / frozen, без причины изменения:
  отличить депозит от своего ордера без догадок нельзя (иначе каждый свой ордер — «внешнее изменение баланса»).
  Зачисление — по `deposit_status` (REST). `venue.balance_events` у CoinEx не действует.
- **Окна (решение 125, window_tap)** — не подключены (контракт отвечает NotApplicable).
- **Удержание вывода (решение 150)** — статуса «предварительно зачислено» нет; есть `deposit_delay_minutes` без описания
  смысла (в журнал) → `withdraw_hold` пусто.
- **Скидка CET** — только глобальный выключатель, прочитать нельзя, размер не описан: расчёт по обычной ставке, факт — из
  ордера (discount_fee → монета из fee_ccy сделок).
- **Единицы ставки trade-fee-rate** (пример «0.03» против «0.002» у пары) — `venue.fee_rate_percent` по живому ответу.
- **Имя сети BSC** в двух справочниках — `venue.bsc_network` и `venue.bsc_contract_chain` по живым ответам; адрес из
  ссылки обозревателя не разбирается.
- **Права ключа основного аккаунта** API не отдаёт — «Проверить подключение» говорит об этом прямо.
- **Поиска ордера по client_id нет** — при неизвестном итоге постановки: отмена по client_id и «итог неизвестен».
- **deflation_rate ≠ 0** у сети BSC — смысл не описан: комиссия вывода неизвестна, вывода нет.
- Решение 232: котировка (`book.rs`) — комиссия покупки в монете (пример Get Order Status: buy, base_fee ≠ 0), продажи в USDT
  (не из документации); скидка CET не читается — расчёт без неё. Факт по сделкам (fee_ccy) в нескольких монетах — список частей.
- **checksum стакана** не сверяется (область и знак числа не однозначны) — проверить на живой записи.

## Что важно знать

- WebSocket: адрес — `venue.ws_url`; кадры сжаты (вид — по заголовку данных, пишется в журнал один раз);
  `server.ping` раз в `venue.ws_ping_ms` (интервал в документации не указан); стакан пушится только при изменении, полный —
  раз в минуту: `venue.book_stale_ms` — больше 60 000 мс. Лимиты подписок и соединений не описаны (ошибка 23002) — один
  запрос подписки на соединение, `venue.ws_symbols_per_conn` — по живому подключению.
- Ордер IOC без исполнения CoinEx не хранит: «не найден» после постановки — проверка сделок пары; нет сделок и в ответе
  постановки filled_amount = 0 — «не исполнился», иначе «итог неизвестен».
- Функции контракта только для DEX отвечают `NotApplicable`.
