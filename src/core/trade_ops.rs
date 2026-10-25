//! Файл: core/trade_ops.rs
//! Что делает: одна покупка или продажа через шлюз (шаг 5 ТЗ) и оценка «сколько получим, если продать
//! сейчас». Худшая цена: у биржи — самый глубокий уровень текущего стакана, нужный на весь объём
//! (лимитный ордер по уровням); у DEX — цена котировки с допуском exec.dex_slippage_pct (минимум на выходе).
//! Ошибка — по правилу шлюза (rules_apply.rs): повтор, только если так задано правилом; «не исполнился» —
//! тоже ошибка (стартовое правило: отменить, не повторять). Газ обмена — в статистику монеты.
//! Кто вызывает: executor.rs, stuck.rs, manual.rs.
//! Худшая цена — worst.rs (та же у расчёта, решение 66).
//! От чего зависит: engine.rs, worst.rs, rules_apply.rs, gas_stats.rs, convert.rs, risk.rs.
//! Покупка с бюджетом (решение 66): qty × худшая цена (лимит биржи, допуск DEX) больше бюджета — ордер не ставится.
//! Настройки: exec.dex_slippage_pct (по площадке DEX), venue.quote_asset.

use std::time::Duration;

use rust_decimal::Decimal;

use crate::core::types::{
    Asset, DexRoute, Quote, Side, TradeRequest, TradeResult, TradeStatus, VenueId,
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

pub use super::worst::worst_price;

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
    let op = if side == Side::Buy {
        "Покупка"
    } else {
        "Продажа"
    };
    let mut attempt = 0;
    loop {
        attempt += 1;
        let worst = worst_price(core, v, a, side, qty, avg)?;
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
            asset: a.clone(),
            side,
            qty,
            worst_price: worst,
            deal_id: deal.to_string(),
            route: route.clone(),
        };
        let res = match core.inner.deps.gw.get(v).await {
            Ok(x) => x.trade(req).await,
            Err(e) => Err(e),
        };
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
            Next::Stop => return Err(OpError::from(&err)),
        }
    }
}

/// Сколько получим чистыми (USDT), если продать qty сейчас: котировка × количество минус затраты
/// продажи вне цены. Возвращает: (сумма, котировка). Ошибки: нет котировки или глубины, нет курса затрат.
pub async fn sell_value(
    core: &Core,
    v: &VenueId,
    a: &Asset,
    qty: Decimal,
) -> Result<(Decimal, Quote), String> {
    let x = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
    let q = x
        .quote(a, Side::Sell, qty)
        .await
        .map_err(|e| format!("{}: {}", v.0, e.message))?;
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
