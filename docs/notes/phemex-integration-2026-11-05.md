# Phemex — интеграция (решение 168): как закрыты 30 уроков

Claude, 05.11.2026. Адаптер `src/venues/phemex/` (только спот), анализ — `phemex-analysis-2026-11-05.md`. Источник —
исходники официального сайта документации (github.com/phemex-docs/phemex-docs.github.io, коммит 98ded36), старая
официальная документация (github.com/phemex/phemex-api-docs) и официальный пример клиента (phemex-python-api). Живых
ответов нет (api.phemex.com из среды разработки закрыт) — всё «проверить на живом» собрано в README адаптера (16
пунктов) и в tests/README.md (раздел Phemex, команды записи).

| # | Урок | Как закрыт |
|---|---|---|
| 1 | Только документация | У каждого файла — ссылки на разделы (#якоря phemex-docs.github.io); чего нет — настройка без значения или отказ: venue.pair_status_listed, venue.bsc_network, venue.book_seq_strict, venue.deposit_to_spot |
| 2 | Противоречия | Success/Succeed (transfers.rs), amount/amountEv (history.rs), quoteCurrency/currency у fee-rate (fees.rs), oderId → поиск только по clOrdID (order_state.rs), depositTo/accountType (deposits.rs) — README, п. 1–16 |
| 3 | Тесты на записях | tests/phemex_real_data.rs: подпись — примеры документации и вызовы официального клиента; разбор — `#[ignore]` до записи |
| 4 | Одна функция правил | order_rules::ready — у trade.rs и check_order (mod.rs); шаги Ev/Ep, ровный перевод в масштаб (num::scale_up), maxBaseOrderSize, min/maxOrderValue; пробного ордера нет → venue.order_test_first = да — ошибка |
| 5 | IOC | timeInForce ImmediateOrCancel / FillOrKill (order_rules.rs: TIF_ALLOWED); остаток — отмена по clOrdID (order_state::settle) |
| 6 | Новый ID попытки | clOrdID — случайный UUID v4 на каждый вызов ready (order_rules::uuid4) |
| 7 | Итог и сроки | by-order-id по clOrdID: cumBaseValueEv, cumQuoteValueEv, cumFeeEv, feeCurrency; сроки venue.order_poll_ms / order_final_wait_ms и venue.history_rows проверяются до отправки |
| 8 | Адрес контракта, имя сети | contractAddress из chainCfg и asset/info, разные — сеть пропускается (networks_parse.rs); venue.bsc_network |
| 9 | Большой справочник | /public/products — разбор в spawn_blocking (symbols.rs); сети — по монете фоновым кругом (networks_crawl.rs), кэш за Arc |
| 10 | Ввод/вывод, memo, счёт | статусы Active/Suspend, minAmountRv, withdrawFeeRv, tag адреса → отказ (deposit_addr.rs); CONTRACT → SPOT — fund_move.rs по venue.deposit_to_spot |
| 11 | Комиссия по паре | fee-rate по quoteCurrency фоном (fees.rs), котировка — из памяти |
| 12 | Стакан, лимиты, пинг | снимок + incremental, рост sequence (строго +1 — venue.book_seq_strict), разрыв — до снимка ≤ 60 с без ресинк-круга (local_book.rs); ≤ 20 подписок, ≤ 5 соединений, ≤ 20 запросов/с, ping < 30 с (market.rs, ws.rs) |
| 13 | Минимум выделений | уровни — i64 в BTreeMap, в Decimal только верхние venue.depth_levels (local_book::top) |
| 14 | Пересподписка | подписка только при подключении; круг ставок не перезапускается при том же списке (subs.rs, решение 131) |
| 15 | REST-лимиты | 429 → пауза venue.rate_limit_pause_ms (reply.rs); группа Others 100/мин — темп сетей venue.networks_requests_per_min |
| 16 | Приватный WS | user.auth + wo.subscribe (user_stream*.rs, wallet_update.rs) по venue.balance_events |
| 17 | Пять частей | код; настройки (catalog/phemex.rs, base.rs); UI — площадка в «Площадках» из реестра, секреты phemex.*; комментарии; ARCHITECTURE — координатор |
| 18 | Секреты | phemex.api_key, phemex.api_secret (secrets::KNOWN, SECURITY.md); подпись входа в журнал не пишется |
| 19 | Права ключа | права через API не читаются (запроса нет); отдельного права «только спот» нет — SECURITY.md; белый список адресов — README |
| 20 | Ядро не меняется | изменены только регистрация (main.rs, venues/mod.rs), секреты, каталог настроек, имя площадки в notify/human.rs |
| 21 | Минимум депозита | minAmountRv приходит фоновым кругом сетей заранее; «0»/пусто → venue.deposit_min_absent_ok |
| 22 | Удержание вывода | статуса «зачислено, вывод нельзя» в документации нет — withdraw_hold по умолчанию (deposits.rs, шапка); Confirmed ≠ Success |
| 23 | Скидка своей монетой | в документации нет — обычная ставка, монета комиссии — feeCurrency ордера |
| 24 | Все счета | Спот, Маржа, USDT-M, UTA; coin-M и Earn — строкой с причиной (all_balances.rs) |
| 25 | Адрес депозита | кэш по (монета, chainName), запрос в предпроверке (deposit_addr.rs) |
| 26 | Повтор круга комиссий | venue.fee_retry_s (fees.rs) |
| 27 | «Счёт не открыт» | кодов нет — ошибка счёта в его error (all_balances.rs) |
| 28 | gzip, IPv4/IPv6 | gzip — общий клиент; venue.api_ip у Phemex не сверяется (IP ключа API не отдаёт) |
| 29 | Своя запись ключа | запроса о ключе нет — «Проверить подключение» пробными вызовами (account.rs) |
| 30 | IOC на живом | IOC в перечне документации; подтвердить первым живым ордером (tests/README.md) |

Вопросы Тиграну: запись живых ответов на сервере; потолок ~100 стаканов; ключ без отдельного права «спот»; адрес
кошелька бота в «withdrawal address list»; VIP-адрес vapi.phemex.com.
