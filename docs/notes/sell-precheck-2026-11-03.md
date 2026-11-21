# Обе стороны до покупки — проверка продажи на B (решение 134, 03.11.2026)

## Живой случай

Сделка `20261103201513697WKC`, направление `bsc_dex → mexc`: WKC куплен на DEX за 52.69 USDT, переведён на MEXC,
продажа на MEXC отклонена — «тип ордера IMMEDIATE_OR_CANCEL (venue.order_type) не в orderTypes пары»
(`venues/mexc/trade.rs`; сам баг закрыт решением 133). Монета застряла, продана вручную через 8 минут, итог −7.98 USDT.

Суть: предпроверка (шаг 5 ТЗ) проверяла сторону покупки и статусы перевода, а сторону продажи B — только по
справочнику пар (флаг API-продажи, `pairs_fresh.rs`) и по расчёту (минимумы, `costs_quote.rs`). Правила самого ордера
адаптера (тип ордера, округление, пробный ордер) впервые применялись к продаже, когда деньги уже потрачены.

## Что сделано

- Контракт шлюза: `check_order(&TradeRequest) -> GatewayResult<()>` — «проверить ордер без исполнения». По умолчанию
  `NotApplicable` → ядро считает это отказом (не торгуем вслепую).
- У бирж правила ордера вынесены в `order_rules.rs` (функция `ready`): её зовёт настоящий ордер (`trade.rs`) и
  проверка (`mod.rs → check_order`). Тест `trade_and_check_share_rules` следит, что в `trade.rs` своих проверок нет.
- Ядро: `core/precheck_sell.rs` — после пересчёта с финальной котировкой (`precheck.rs`, точечная вставка), если B —
  биржа: ордер продажи на `plan.sell_qty` (приход на B после налогов и комиссий, вниз до шага пары) по худшей цене
  `worst.rs` (та же, что поставит исполнение) → `check_order` у B. Время шага — в журнале предпроверки.

## Что проверяется до покупки

| Биржа B | Правила (как у настоящего ордера) | Пробный ордер |
| --- | --- | --- |
| MEXC | пара в selfSymbols ключа, isSpotTradingAllowed, tradeSideType, status 1; тип `venue.order_type` по orderTypes (IOC/FOK — при LIMIT, решение 133); количество вниз до 10^−baseAssetPrecision, цена вверх до 10^−quotePrecision (решение 107); > 0; baseSizePrecision; quoteAmountPrecision; maxQuoteAmount; заданы сроки итога (order_state) и `venue.my_trades_limit` | `POST /api/v3/order/test` при `venue.order_test_first` = да (https://www.mexc.com/api-docs/spot-v3/spot-account-trade/test-new-order) — ловит и PERCENT_PRICE_BY_SIDE |
| BingX | status 1 и apiStateSell; количество вниз до stepSize, цена вверх до tickSize; > 0; minQty / maxQty; minNotional / maxNotional (0 у максимума — предел неизвестен); задан `venue.order_time_in_force` | **нет у спота** — в официальном справочнике спотовой торговли (https://github.com/BingX-API/api-ai-skills/blob/main/skills/spot-trade/api-reference.md, список — skills/spot-trade/SKILL.md) только order, batchOrders, cancel*, cancelReplace, query, openOrders, historyOrders, myTrades; `order/test` есть только у бессрочных фьючерсов (`/openApi/swap/v2/trade/order/test`), фьючерсов в проекте нет |
| DEX (bsc_dex) | не меняется: исполнимость продажи — котировка, маршрут и проверка в пересчёте (`profit.rs`); `check_order` не вызывается | — |

Новое у настоящего ордера BingX: minQty / maxQty пары теперь соблюдаются и у ордера (раньше только у расчёта,
решение 66) — следствие «одна функция правил».

## Отказ и риски

Отказ — `Err(String)` предпроверки: исполнитель пишет в журнал `core.precheck` «сделки нет — продажа на B не пройдёт:
… (OrderRejected)» и выходит; сделка не открывается, денег не тронуто.

Счётчик `risk.max_errors_in_row` растёт только в `risk::on_error`, а его зовёт только `rules_apply::on_error` —
ошибки операций с деньгами (ордер в `trade_ops.rs`, вывод `transfers.rs`, зачисление `transfer_credit.rs`, цикл
`cycle_deal.rs`). Предпроверка (`precheck.rs` и `precheck_sell.rs`) их не зовёт — отказ до сделки не ставит площадку
на паузу (как и прочие отказы предпроверки: статусы перевода, адрес депозита, BNB на газ). Сброс счётчика
(`risk::on_ok`) — тоже только после исполненного ордера. Отказ пробного ордера ВНУТРИ настоящего ордера (trade.rs)
по-прежнему идёт через правила шлюза и считается.

## Что осталось

ROADMAP «Техдолг», решение 134: проверка не повторяется после подбора объёма под остаток (`refit.rs`); покупка на
бирже A через `check_order` не проверяется (её отказ и так до траты денег); проверка — один REST MEXC последовательно.
