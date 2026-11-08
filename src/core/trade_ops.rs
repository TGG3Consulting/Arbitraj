//! Файл: core/trade_ops.rs
//! Что делает: одна покупка или продажа через шлюз (шаг 5 ТЗ) и оценка «сколько получим, если продать
//! сейчас». Худшая цена: у биржи — самый глубокий уровень текущего стакана, нужный на весь объём
//! (лимитный ордер по уровням); у DEX — цена котировки с допуском exec.dex_slippage_pct (минимум на выходе).
//! Ошибка — по правилу шлюза (rules_apply.rs): повтор, только если так задано правилом; «не исполнился» —
//! тоже ошибка (стартовое правило: отменить, не повторять). Газ обмена — в статистику монеты.
//! Кто вызывает: executor.rs, stuck.rs, manual.rs; deal_value / sell_value — sell.rs, sell_unsent.rs, relay*.rs,
//! partial.rs, daily_value.rs (решение 220: котировка кошельком сделки).
//! Худшая цена — worst.rs (та же у расчёта, решение 66); у ордера — только по свежему стакану (решение 227).
//! Решение 237: котировка исполнения биржи (sell_value) и худшая цена ордера — один стакан (exec_book.rs).
//! От чего зависит: engine.rs, worst.rs, rules_apply.rs, gas_stats.rs, convert.rs, risk.rs, outcome_resolve.rs.
//! Покупка с бюджетом (решение 66): qty × худшая цена (лимит биржи, допуск DEX) больше бюджета — ордер не ставится.
//! Решение 177: у продажи — признак выхода из позиции (TradeRequest.exit_position, продажа DEX по квотеру).
//! Решение 231: «исход неизвестен» с «чем найти» — после паузы площадки бот ищет ордер сам (outcome_resolve.rs); найден
//! окончательный итог — разбирается как обычный ответ на ордер; не найден за срок — как раньше, ручная проверка.
//! Настройки: exec.dex_slippage_pct (по площадке DEX), venue.quote_asset; risk.outcome_poll_s, risk.outcome_max_s
//! (через outcome_resolve.rs).

use std::time::Duration;

use rust_decimal::Decimal;

use crate::core::types::{
    Asset, DexAccountId, DexRoute, Quote, Side, TradeRequest, TradeResult, TradeStatus, VenueId,
};
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::read::At;

use super::convert;
use super::engine::{Core, rd};
use super::op_error::OpError;
use super::rules_apply::{self, Next};

/// Вид площадки из реестра. Ошибка: площадки нет.
pub fn kind(core: &Core, v: &VenueId) -> Result<VenueKind, String> {
    rd(&core.inner.kinds)
        .iter()
        .find(|(id, _)| id == v)
        .map(|(_, k)| *k)
        .ok_or_else(|| format!("площадки {} нет в реестре", v.0))
}

pub use super::worst::{Worst, worst_price};

/// Покупка или продажа. Принимает: ядро, id сделки, площадку, монету, сторону, количество,
/// маршрут DEX из котировки, среднюю цену котировки, бюджет покупки (решение 66: qty × худшая цена не больше его —
/// иначе ордер не ставится; None — без предела). Возвращает: факт (исполнено полностью или частично).
/// Ошибки: текст причины (не исполнился, отклонён, нет данных, бюджет) — уже в журнале и уведомлениях.
#[allow(clippy::too_many_arguments)]
pub async fn trade(
    core: &Core,
    deal: &str,
    v: &VenueId,
    a: &Asset,
    side: Side,
    qty: Decimal,
    route: Option<DexRoute>,
    avg: Decimal,
    cap: Option<Decimal>,
) -> Result<TradeResult, OpError> {
    let mut seen = None;
    trade_seen(core, deal, v, a, side, qty, route, avg, cap, &mut seen).await
}

/// Как trade, плюс по чему посчитана худшая цена последней попытки (решение 227: продажа пишет её в хронологию
/// сделки — sell.rs). Принимает: как trade и место для Worst (None — до расчёта худшей цены не дошло).
/// Ошибки: как trade; «стакан устарел» (worst.rs worst_order).
#[allow(clippy::too_many_arguments)]
pub async fn trade_seen(
    core: &Core,
    deal: &str,
    v: &VenueId,
    a: &Asset,
    side: Side,
    qty: Decimal,
    route: Option<DexRoute>,
    avg: Decimal,
    cap: Option<Decimal>,
    seen: &mut Option<Worst>,
) -> Result<TradeResult, OpError> {
    let op = if side == Side::Buy {
        "Покупка"
    } else {
        "Продажа"
    };
    let mut attempt = 0;
    // Решение 231: итог операции с неизвестным исходом, найденный ботом (outcome_resolve.rs), — разбирается ниже как
    // ответ на ордер (тот же путь: имена монет, газ, «не исполнилась» по правилам шлюза), без новой постановки.
    let mut found: Option<TradeResult> = None;
    loop {
        let (x, req, res) = match found.take() {
            Some(r) => (None, r.request.clone(), Ok(r)),
            None => {
                attempt += 1;
                // Решение 237: у ордера биржи — тот же стакан и та же мера свежести, что у котировки исполнения
                // (exec_book.rs: стакан адаптера или снимок REST, если поток устарел); REST не ответил после повтора —
                // ошибка без кода безопасного повтора (алерт, а не повторы по тому же стакану).
                let w = super::worst::worst_order(core, v, a, side, qty, avg).await?;
                *seen = Some(w);
                let worst = w.price;
                // Решение 66: худшая цена берётся заново по живому стакану — списание не должно выйти за бюджет.
                if let Some(cap) = cap
                    && side == Side::Buy
                    && qty * worst > cap
                {
                    return Err(OpError::from(format!(
                        "{op} {qty} {}: списание по худшей цене {worst} — {} больше бюджета {cap}, ордер не ставится",
                        a.ticker,
                        qty * worst
                    )));
                }
                let req = TradeRequest {
                    venue: v.clone(),
                    // Решение 211: ордер площадки — с её тикером монеты; результат — обратно в монету ядра (ниже).
                    asset: super::venue_ticker::on_venue(core, a, v),
                    side,
                    qty,
                    worst_price: worst,
                    deal_id: deal.to_string(),
                    route: route.clone(),
                    // Решение 177: каждая продажа отсюда — выход из позиции: монета уже куплена (продажа сделки —
                    // sell.rs, в том числе продажа на DEX в сделке CEX → DEX; застрявшая монета; кнопки человека;
                    // «Продать» в «Балансах»). Покупка — нет. Циклы DEX ↔ DEX сюда не ходят (cycle_deal.rs, свой
                    // запрос) — у них сверка с квотером — стоп.
                    exit_position: side == Side::Sell,
                };
                // Решение 132: DEX — кошелёк сделки.
                match super::dex_account::venue_deal(core, deal, v).await {
                    Ok(x) => {
                        let res = x.trade(req.clone()).await;
                        (Some(x), req, res)
                    }
                    Err(e) => {
                        let e = GatewayError::new(ErrorCode::NotConfigured, Some(v.clone()), e);
                        (None, req, Err(e))
                    }
                }
            }
        };
        let res = res.map(|mut r| {
            // Решение 219: каждая монета комиссии — в имя ядра.
            for f in &mut r.fees {
                f.asset = super::venue_ticker::name_back(core, a, v, &f.asset);
            }
            r.request.asset = a.clone();
            r
        });
        let err = match res {
            Ok(r) => {
                if let Some(tx) = &r.chain {
                    super::gas_stats::record_tx(core, a, tx).await;
                }
                if r.status != TradeStatus::NotFilled && r.filled_qty > Decimal::ZERO {
                    super::risk::on_ok(core, v);
                    return Ok(r);
                }
                GatewayError::new(
                    ErrorCode::OrderNotFilled,
                    Some(v.clone()),
                    format!("{op} {qty} {} не исполнилась", a.ticker),
                )
            }
            Err(e) => {
                if let Some(tx) = &e.tx {
                    super::gas_stats::record_tx(core, a, tx).await;
                }
                e
            }
        };
        match rules_apply::on_error(core, op, v, Some(deal), &err, attempt).await {
            Next::Retry { pause_ms } => tokio::time::sleep(Duration::from_millis(pause_ms)).await,
            Next::Stop => {
                // Решение 231: исход неизвестен — бот ищет ордер по «чем найти» из ошибки; нашёл — разбор как ответа.
                // Отклонён площадкой — окончательно, ничего не исполнено: как обычный отказ ордера, без повтора.
                if let Some(x) = x.as_ref().filter(|_| err.code == ErrorCode::OutcomeUnknown)
                    && let Some(r) =
                        super::outcome_resolve::trade(core, op, deal, v, x, &req, &err).await
                {
                    match r {
                        Ok(r) => {
                            found = Some(r);
                            continue;
                        }
                        Err(rejected) => {
                            // Газ найденного факта (откат, отмена, занявшая nonce) — в статистику, как обычный путь.
                            if let Some(tx) = &rejected.tx {
                                super::gas_stats::record_tx(core, a, tx).await;
                            }
                            return Err(OpError::from(&rejected));
                        }
                    }
                }
                return Err(OpError::from(&err));
            }
        }
    }
}

/// Решение 220: сколько получим, если продать ВСЕ монеты сделки там, где они лежат (deal.at), — котировка кошельком
/// СДЕЛКИ (dex_account::of). Так маршрут, который этот кошелёк не соберёт (межроутерный без исполнителя), не выбирается.
/// Принимает: ядро, сделку. Возвращает: как sell_value. Ошибки: как sell_value; сделка с DEX без счёта (dex_account::of).
pub async fn deal_value(core: &Core, deal: &super::deal::Deal) -> Result<(Decimal, Quote), String> {
    let acc = super::dex_account::of(core, deal)?;
    sell_value(
        core,
        acc.as_ref(),
        &deal.at,
        &deal.direction.asset,
        deal.qty,
    )
    .await
}

/// Сколько получим чистыми (USDT), если продать qty сейчас: котировка × количество минус затраты
/// продажи вне цены. Решение 220: котировка DEX — контекстом кошелька `acc`, которым пойдёт продажа (его исполнитель
/// решает, исполним ли межроутерный маршрут; без счёта котировал кошелёк строки «Пробы» — живые HOLO, PORTO, ATA:
/// маршрут выбран под исполнитель «Проб», а продажа кошельком сделки без исполнителя ушла в бандл и отказала).
/// Принимает: ядро, счёт DEX (None — биржа или DEX не участвует), площадку, монету, количество.
/// Возвращает: (сумма, котировка). Ошибки: нет котировки или глубины, нет курса затрат, нет кошелька.
pub async fn sell_value(
    core: &Core,
    acc: Option<&DexAccountId>,
    v: &VenueId,
    a: &Asset,
    qty: Decimal,
) -> Result<(Decimal, Quote), String> {
    let x = core
        .inner
        .deps
        .gw
        .get_as(v, acc)
        .await
        .map_err(|e| e.message)?;
    // Решение 237: котировка исполнения у биржи — стакан для ордера (exec_book.rs): тот же снимок и та же мера
    // свежести, что у худшей цены ордера (worst.rs); стакан потока устарел — снимок REST. У DEX — котировка для
    // исполнения (gateway/patience.rs exec_scope), как раньше.
    // Решение 211: площадке — её тикер монеты; ответ — обратно в монету ядра.
    let q = if kind(core, v)? == VenueKind::Cex {
        let b = super::exec_book::book(core, v, a, Side::Sell)
            .await
            .map_err(|e| e.message)?;
        b.snap
            .quote(qty)
            .await
            .map_err(|e| format!("{}: {}", v.0, e.message))?
    } else {
        let on = super::venue_ticker::on_venue(core, a, v);
        crate::gateway::patience::exec_scope(x.quote(&on, Side::Sell, qty))
            .await
            .map_err(|e| format!("{}: {}", v.0, e.message))?
    };
    let q = super::venue_ticker::quote_back(core, a, v, q);
    if q.fillable_qty < qty {
        return Err(format!(
            "{}: глубины хватает на {} из {qty}",
            v.0, q.fillable_qty
        ));
    }
    let quote = core
        .inner
        .deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&v.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let cex: Vec<VenueId> = rd(&core.inner.kinds)
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| v.clone())
        .collect();
    let priced: Vec<_> = q.costs.iter().filter(|c| !c.in_price).cloned().collect();
    let rates = convert::rates(
        &core.inner.deps,
        &cex,
        vec![quote],
        &a.ticker,
        q.avg_price,
        &priced,
    )
    .await?;
    let mut net = qty * q.avg_price;
    for c in q.costs.iter().filter(|c| !c.in_price) {
        net -= convert::to_usdt(c, &rates)?;
    }
    Ok((net, q))
}
