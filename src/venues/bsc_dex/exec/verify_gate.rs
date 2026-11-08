//! Файл: venues/bsc_dex/exec/verify_gate.rs
//! Что делает: предторговая верификация графового маршрута точечным Multicall3 (решение 83, фаза 80.6b) за правилом
//! шлюза. Перед формированием сделки по маршруту RouteMode::Graph восстанавливает план-котировку из запроса (среднюю
//! цену — обратной формулой core/worst.rs: worst = avg × (1 ± exec.dex_slippage_pct)), перечитывает пулы маршрута на
//! свежем блоке (quote/verify.rs run; граница сделки req.worst_price передаётся туда же — решение 121 B4: отказ, только
//! если свежая средняя цена хуже неё) и переводит вердикт в решение шлюза: Reject → ошибка (QuoteMismatch — цена
//! уехала; StaleData — свежесть/чтение/таймаут), ордер НЕ формируется (стартовое правило Тиграна «не исполнился —
//! отменяем, больше не пробуем»: здесь не отправляем вовсе); Ok → свежие amountOutMin по ногам (решение 84), которые
//! ставятся последнему шагу каждой доли нижней границей (exec/leg_min.rs set_leg_mins, решение 228: минимум доли =
//! E_k × max(1 − допуск, B / E); минимум верификации выше ожидаемого доли — отказ). Итог — в уведомления
//! (exec.verified / exec.verify_rejected): видно из интерфейса и в Telegram, если событие включено.
//! Классику (прямой/умный путь без графа) не трогает — вызывается только из exec/trade.rs для RouteMode::Graph.
//! Кто вызывает: exec/trade.rs (графовый маршрут); повтор после approve — exec/{trade_direct, trade_smart}.rs,
//! exec/executor/trade.rs; plan_quote / outcome — exec/dry_paths.rs.
//! От чего зависит: quote/verify.rs, quote/via.rs, quote/steps.rs, quote/direct.rs, exec/trade.rs (sides),
//! exec/txs.rs (Env), send/mod.rs (planned_floor), core/types.
//! Настройки: exec.dex_slippage_pct [bsc_dex] (для восстановления цены плана); verify.* — через quote/verify.rs.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Quote, Side, TradeRequest};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::ctx::Ctx;
use super::super::quote::steps::Names;
use super::super::quote::verify::{self, Verdict};
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::send;
use super::txs::Env;

/// Итог успешной верификации графового маршрута: свежие минимумы выхода по ногам (решение 84) и блок перечтения.
#[derive(Debug, Clone)]
pub struct Verified {
    pub amount_out_min: Vec<Decimal>,
    pub block: u64,
}

/// Верификация графового маршрута перед сделкой. Принимает: окружение котировки, окружение отправки (для уведомления),
/// запрос, маршрут из запроса. Возвращает: свежие минимумы и блок. Ошибки: NotConfigured (exec.dex_slippage_pct,
/// verify.*), QuoteMismatch (цена уехала), StaleData (свежесть/чтение/таймаут), NoRoute, узел.
pub async fn gate(
    q: &Quoting,
    env: &Env,
    req: &TradeRequest,
    route: &DexRoute,
) -> GatewayResult<Verified> {
    let ctx = &q.ctx;
    let (coin, usdt, decs) = super::trade::sides(q, req).await?;
    let taxes = direct::taxes(q, &req.asset)?;
    // Допуск проскальзывания — через ? (конвенция боевого пути exec/order.rs): не задан → NotConfigured, ордера нет.
    let slip_pct = ctx.dec("exec.dex_slippage_pct")?;
    let plan = plan_quote(ctx, req, route, slip_pct)?;
    let gas = q
        .gas
        .wei(ctx, &q.heads)
        .await?
        .max(send::planned_floor(ctx)?);
    let usdt_ticker = ctx.text("venue.quote_asset")?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let names = Names {
        usdt: (usdt, &usdt_ticker),
        coin: (coin, &req.asset.ticker),
        mids: &q.mids,
    };
    // Граница сделки ядра (req.worst_price) — прямо в вердикт (решение 121 B4): отказ, только если свежая цена хуже её.
    let verdict = verify::run(&v, &names, &plan, gas, req.worst_price).await?;
    announce(env, ctx, req, &verdict);
    outcome(ctx, verdict)
}

/// План-котировка из запроса: средняя цена восстановлена из худшей допустимой и exec.dex_slippage_pct — обратная
/// формула core/worst.rs (worst = avg × (1 ± slip)), та же настройка, что ставит худшую цену и минимум боевого пути.
/// Остальное — из запроса (маршрут, монета, сторона, количество). Ошибки: NotConfigured (slip ≥ 100%).
pub fn plan_quote(
    ctx: &Ctx,
    req: &TradeRequest,
    route: &DexRoute,
    slip_pct: Decimal,
) -> GatewayResult<Quote> {
    let slip = slip_pct / Decimal::ONE_HUNDRED;
    let denom = match req.side {
        Side::Buy => Decimal::ONE + slip,
        Side::Sell => Decimal::ONE - slip,
    };
    if denom <= Decimal::ZERO {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "exec.dex_slippage_pct ≥ 100% — средняя цена плана не восстанавливается",
        ));
    }
    Ok(Quote {
        venue: ctx.venue(),
        asset: req.asset.clone(),
        side: req.side,
        qty: req.qty,
        fillable_qty: req.qty,
        avg_price: req.worst_price / denom,
        costs: Vec::new(),
        route: Some(route.clone()),
        signal_route: None,
        pick: None,
        at: Utc::now(),
    })
}

/// Вердикт → решение шлюза. Ok → свежие минимумы; Reject → ошибка (ордер не формируется): по дрейфу — QuoteMismatch,
/// по свежести/чтению/таймауту (дрейфа нет) — StaleData. Чистая, тестируется. Ошибки: QuoteMismatch / StaleData.
pub fn outcome(ctx: &Ctx, verdict: Verdict) -> GatewayResult<Verified> {
    match verdict {
        Verdict::Ok {
            amount_out_min,
            block,
            ..
        } => Ok(Verified {
            amount_out_min,
            block,
        }),
        Verdict::Reject { reason, drift } => {
            let code = if drift.is_some() {
                ErrorCode::QuoteMismatch
            } else {
                ErrorCode::StaleData
            };
            Err(ctx.err(
                code,
                format!("предторговая верификация маршрута: {reason} — ордер не формируется"),
            ))
        }
    }
}

/// Уведомление о результате верификации (видно из интерфейса и в Telegram, если событие включено). Ошибок нет.
fn announce(env: &Env, ctx: &Ctx, req: &TradeRequest, verdict: &Verdict) {
    let op = ["Продажа", "Покупка"][usize::from(req.side == Side::Buy)];
    let (code, text) = match verdict {
        Verdict::Ok {
            amount_out_min,
            block,
            ..
        } => {
            let mins = amount_out_min
                .iter()
                .map(|m| m.normalize().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            (
                "exec.verified",
                format!(
                    "{op} {}: маршрут верифицирован на блоке {block}, минимум выхода по ногам [{mins}]",
                    req.asset.ticker
                ),
            )
        }
        Verdict::Reject { reason, .. } => (
            "exec.verify_rejected",
            format!(
                "{op} {}: маршрут отклонён предторговой верификацией — {reason}; ордер не формируется",
                req.asset.ticker
            ),
        ),
    };
    env.stats.emit(MarketEvent::Notice {
        venue: ctx.venue(),
        code: code.to_string(),
        text,
    });
}
