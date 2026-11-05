# Kraken — интеграция (решение 172): как закрыт каждый из 30 уроков

Claude, 05.11.2026. Адаптер `src/venues/kraken/` (README папки), анализ — `kraken-analysis-2026-11-05.md`, уроки —
`cex-integration-lessons-2026-11-04.md`. Из среды разработки Kraken закрыт прокси: страницы документации прочитаны
пересказчиком (подпись Funding, методы, вывод, OpenAPI-пути /funding, book, чек-сумма, balances), живых ответов нет.
Таблица ошибок Kraken (страница errors) из среды не открылась — строки ключа/подписи/прав/баланса в errors.rs сверить.

| # | Как закрыт |
|---|---|
| 1 | Ссылка на раздел документации — в шапке каждого файла и у функций (`rest.rs`, `methods.rs`, `transfers.rs`, …) |
| 2 | Расхождения: GetApiKeyInfo (пример camelCase, схема snake_case) — читаются оба (`account.rs`); EarlyConfirmed ↔ «Credited» и статусы Funding-депозитов — консервативно, вопрос в отчёте |
| 3 | Тесты без моков: подпись — официальный пример гида и формула помощника; разбор — `#[ignore]` до записи (`tests/kraken_real_data.rs`, команды — `tests/README.md`) |
| 4 | Одна функция `order_rules::ready` у `trade` и `check_order`; пробный ордер `validate=true` — `order_rules::probe` при `venue.order_test_first` |
| 5 | `limit` + `timeinforce` IOC/FOK (`venue.order_time_in_force`), остаток снимает биржа (`expired`); не дождались — отмена (`order_state::settle`) |
| 6 | `cl_ord_id` ≤ 18 ASCII, новый на попытку (`order_rules::cl_ord_id`); у вывода своего id нет — поиск по методу, адресу и времени (`transfers::find`), неоднозначно — «итог неизвестен» |
| 7 | Итог — `QueryOrders`: vol_exec, price, fee (котируемая, `oflags=fciq`); сроки `venue.order_poll_ms`/`order_final_wait_ms` проверяются до постановки |
| 8 | Монета по адресу: `network.contract_address` метода Funding только на сети `venue.bsc_network` (`networks_parse.rs`); ввод/вывод с разным адресом — не сопоставляется |
| 9 | Методы — все страницы по курсору или одной монеты (`methods::list`), разбор — `spawn_blocking`, кэш за `Arc` (`wallet.rs`) |
| 10 | Ввод/вывод: метод в списке направления + `Assets.status`; комиссия — `fees.base`, процентная — комиссия неизвестна (ядро не торгует); минимумы методов; точность — `Assets.decimals`; memo — отказ; депозит сразу на спот |
| 11 | Ставки — `TradeVolume` по паре и ключу (fee в процентах), не числом; монета — по факту ордера |
| 12 | Стакан: снимок + изменения, чек-сумма CRC32 топ-10 после кадра; разрыв → пересподписка пары, второй раз раньше паузы → переподключение (`ws_conn.rs`); пинг < 60 с; пачки и темп подписок — настройки |
| 13 | Уровни в `BTreeMap` без копий; ядру — верхние уровни один раз на кадр (`local_book.rs`) |
| 14 | Круг ставок не перезапускается при том же списке пар (`subs.rs`, как OKX) |
| 15 | Без REST на горячем пути; «Rate limit exceeded» — пауза ключа, «EOrder: Rate limit» — пауза ордеров, Throttled — до метки (`reply.rs`); ставки пачками |
| 16 | Приватный WS `balances` (токен на подключение) → `MarketEvent::Balance` (`user_stream_conn.rs`, `balance_event.rs`) |
| 17 | Код + настройки (`settings/catalog/kraken.rs`, `docs/SETTINGS.md`) + интерфейс (площадка на «Площадки», «Проверить подключение») + комментарии; ARCHITECTURE — координатор |
| 18 | Секреты `kraken.api_key`, `kraken.api_secret` (`secrets/mod.rs`, `docs/SECURITY.md`); passphrase нет; токен WS не пишется в журнал |
| 19 | Права проверяются (`account.rs`); лишние права адресной книги — предупреждение; маржу правом не отключить — `leverage` не шлётся |
| 20 | Ядро и шлюз не тронуты; регистрация — `main.rs` |
| 21 | Минимум депозита — `minimum_amount` метода в справочнике; нет — `venue.deposit_min_absent_ok` |
| 22 | `withdraw_hold` — `DepositStatus` со статусом EarlyConfirmed (`deposits.rs`) |
| 23 | Неприменим: скидки своей монетой у Kraken нет |
| 24 | `account_balances`: спот, прочие кошельки (суффиксы), фьючерсы — сведения (`all_balances.rs`) |
| 25 | Адрес депозита — кэш по (монета, сеть) с `expire_time`; новый — только при `venue.deposit_address_claim` (`deposit_addr.rs`) |
| 26 | Повтор круга ставок со сбоями — `venue.fee_retry_s` (`fees.rs`) |
| 27 | Пустой BalanceEx — пустой счёт; ответа «счёт не открыт» в документации нет |
| 28 | gzip — HTTP-клиент; `venue.api_ip` — привязка ключа к IP в проверке подключения |
| 29 | Права — своей записи ключа (`GetApiKeyInfo.permissions`), имена — из спецификации |
| 30 | IOC/`expired` и `validate` — на пробном/живом ордере (команды в `tests/README.md`) |

Новое у Kraken: единый nonce (полоса `ctx.lane`), вывод только по `address_id` с котировкой комиссии, дифф-стакан по
чек-сумме, Funding (Beta) рядом со спот REST, подключения WebSocket — не чаще одного на 4 с (Cloudflare).
