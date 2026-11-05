# HTX — интеграция (решение 171), как закрыты 30 уроков (05.11.2026)

Адаптер `src/venues/htx/` (README.md папки), анализ — `htx-analysis-2026-11-05.md`, чек-лист —
`cex-integration-lessons-2026-11-04.md`. Источники: официальная документация https://huobiapi.github.io/docs/spot/v1/en/
и SDK github.com/HuobiRDCenter/huobi_Golang. Живых ответов нет (api.huobi.pro закрыт прокси) — команды записи и что
проверить: `tests/README.md`, раздел HTX.

| Урок | Как закрыт |
| --- | --- |
| 1 Документация | Каждый файл — шапка с разделом документации; что не описано — настройка без значения или отказ с причиной (README.md папки, «Чего нет») |
| 2 Противоречия | filled-* (документация) / field-* (SDK) — принимаются оба (`order_state.rs`, `filled`); client-order-id 8 ч / 24 ч — новый ID на попытку; пробел %20 (документация) / «+» (SDK) — в параметрах бота пробелов нет (`sign.rs`) |
| 3 Тесты на записях | `tests/htx_real_data.rs`: подпись — векторы тестов SDK; разбор — `#[ignore]` до записи `htx_*` |
| 4 order_rules::ready | `order_rules.rs` `ready` — у `trade.rs` и у `check_order` (`mod.rs`) |
| 5 IOC | Тип buy-ioc / sell-ioc или buy-limit-fok / sell-limit-fok (`order_rules::order_type`); не дождались итога — отмена (`order_state::settle`) |
| 6 Свой ID | `order_rules::client_oid` — новый на попытку, ≤ 64 (ордер), ≤ 32 (вывод, `transfers::MAX_WITHDRAW_CID`) |
| 7 Итог ордера | Деталь ордера + matchresults (`order_state::result`, `order_fee.rs`); сроки venue.order_poll_ms / order_final_wait_ms проверяются до постановки; отменённый виден 2 ч — итог читается сразу |
| 8 Адрес контракта | **Не документирован.** `venue.contract_field` (ac / ca) — без неё `bsc_contract = None`, монеты не сопоставляются, журнал и «Проверить подключение» говорят причину (`networks_parse.rs`, `wallet.rs`, `account.rs`). Сеть BSC — baseChainProtocol = venue.bsc_network |
| 9 Большой справочник | currencies, chains, market-symbols — `heavy` (разбор вне рабочих потоков), кэш за `Arc`, одна монета — ?currency= |
| 10 Ввод / вывод | depositStatus / withdrawStatus, instStatus delisted, комиссия fixed, min/max, withdrawPrecision, numOfConfirmations; addressTag — отказ; депозит на спот (перевода нет) |
| 11 Комиссия по ключу | transact-fee-rate по паре (`fees.rs`), худшая из takerFeeRate / actualTakerRate |
| 12 Стакан | mbp.refresh.5/10/20 снимками, pong на ping сервера, отсев по seqNum; предел тем на соединение не описан — venue.ws_symbols_per_conn |
| 13 Память / CPU | Один разбор JSON на кадр, уровни без копий; GZIP каждого кадра — **замер нагрузки при «Стопе» обязателен** |
| 14 Пересподписка | `subs.rs` как Bitget/MEXC: круг ставок не перезапускается при том же списке |
| 15 REST-лимиты | HTTP 429 — пауза всех запросов (`reply.rs`); ответ на превышение не описан (вопрос); REST на горячем пути нет |
| 16 Поток аккаунта | `user_stream*.rs`, accounts.update#2 → MarketEvent::Balance (`account_update.rs`) |
| 17 Готовая фича | Код, настройки (`settings/catalog/htx.rs` + базовые), «Площадки» и «Проверить подключение», комментарии; ARCHITECTURE — у координатора |
| 18 Секреты | htx.api_key, htx.api_secret — в файле секретов; подпись входа в журнал не пишется |
| 19 Права ключа | Read, Trade, Withdraw (`account.rs`); Trade у HTX общий — фьючерсы не вызываются кодом; список адресов вывода проверяется до вывода (`transfers::whitelisted`) |
| 20 Ядро не меняется | Только регистрация (main.rs), секреты, каталог настроек, имя площадки и коды причин в notify (решение 164) |
| 21 Мин. депозита | minDepositAmt прямо в справочнике; «0» / пусто → venue.deposit_min_absent_ok |
| 22 Удержание вывода | Депозит confirmed — Credited и удержан (`deposits::withdraw_hold`), safe — свободен |
| 23 Скидка своей монетой | HT / HB-баллы: расчёт по худшей ставке, факт — fee-deduct-currency / filled-points; HT кончился — сделки идут |
| 24 Все счета | `all_balances.rs`: каждый счёт списка; фьючерсы — отдельный API (note) |
| 25 Адрес депозита | `deposit_addr.rs` — кэш по (монета, сеть) |
| 26 Комиссии фоном | venue.fee_retry_s после сбоя круга |
| 27 «Счёт не открыт» | Нет спот-счёта — балансы пусто, ордер — NotConfigured с причиной (`accounts.rs`) |
| 28 gzip / IPv4 | REST gzip у клиента; WS рынка — GZIP; IPv4/IPv6 в документации не описан (venue.api_ip — только для сверки ключа) |
| 29 Своя запись ключа | /v2/user/uid → /v2/user/api-key?uid=&accessKey= (permission, status, ipAddresses, validDays) |
| 30 IOC живым | Пробного ордера нет — проверить первым минимальным живым ордером |

## Вопросы Тиграну

1. Поле адреса контракта (ac / ca) и имя сети BSC (baseChainProtocol) — по живому ответу; если адреса нет — HTX не подключать.
2. Комиссия вывода circulated / ratio — считать максимумом или не выводить (сейчас — не выводить)?
3. Вывод confirm-error — ждать или провал (сейчас — ждать)?
4. Входит ли fee в amount вывода — проверить малым выводом.
5. Ответ REST на превышение лимита; предел тем на соединение WS; символов в transact-fee-rate.
6. Скидка HT — включать на счёте бота? (в расчёт не входит в любом случае).
