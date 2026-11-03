//! Файл: venues/bsc_dex/exec/live_hops.rs
//! Что делает: живой путь доли графового маршрута из ТРЁХ и более пулов (решение 122, часть R2) прямо перед сделкой —
//! та же математика N хопов, что у котировки графа (quote/route_calc.rs). Токены хопов — цепочкой от старта стороны
//! (покупка — USDT, продажа — монета): выход хопа — другой конец его пула; цепочка обязана закончиться токеном
//! выхода сделки. Перед расчётом — повторная проба крючка B/C каждого пула (решение 32, exec/precheck.rs) и допуск
//! пула по его стороне на этом хопе (quote/select.rs, как quote/graph_legs.rs). Те же правила, что у пары через
//! промежуточную (exec/live.rs, решения 39, 52): внутренние токены — включённые промежуточные монеты без налогов;
//! у монеты с налогом — только пулы V2 на каждом шаге. Путь из 1–2 пулов считает exec/live.rs по-прежнему.
//! Кто вызывает: exec/live.rs.
//! От чего зависит: exec/precheck.rs, quote/route_calc.rs, quote/graph_legs.rs (hop_side), quote/select.rs,
//! quote/via.rs (Via: mid_ok, taxed), pools/registry.rs, pools/mids.rs, tokens.rs.
//! Настройки: через вызываемые — см. их шапки.

use rust_decimal::Decimal;

use crate::core::types::{PoolKind, RouteHop, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::quote::path::Path;
use super::super::quote::route_calc::{self, Leg};
use super::super::quote::via::Via;
use super::super::quote::{graph_legs, select};
use super::precheck;

/// Живой путь доли из N пулов. Принимает: расчёт путей (монета, сторона, налоги, decimals), пулы доли в порядке
/// исполнения, количество монет доли. Ошибки: NoRoute (пул пропал / не допущен / цепочка токенов не сходится /
/// внутренний токен не промежуточная / налог), CoinNotChecked, QuoteMismatch, BadResponse, узел (decimals).
pub async fn path(v: &Via<'_>, hops: &[RouteHop], qty: Decimal) -> GatewayResult<Path> {
    let (q, ctx) = (v.q, &v.q.ctx);
    let no = |w: String| ctx.err(ErrorCode::NoRoute, w);
    let (start, end) = match v.side {
        Side::Buy => (v.usdt, v.coin),
        Side::Sell => (v.coin, v.usdt),
    };
    let mids = q.mids.active(ctx);
    let mut running = start;
    let mut legs = Vec::with_capacity(hops.len());
    for (i, h) in hops.iter().enumerate() {
        let e = q
            .reg
            .get(&h.pool)
            .ok_or_else(|| no("Пул маршрута больше не в реестре".into()))?;
        precheck::hook_again(q, &e).await?;
        let token_in = running;
        let token_out = if e.usdt == token_in {
            e.coin
        } else if e.coin == token_in {
            e.usdt
        } else {
            return Err(no(format!(
                "{}: пул не соединяет токены хопа {} — цепочка доли разорвана",
                e.family.title,
                i + 1
            )));
        };
        // Внутренний токен (выход не последнего хопа) — включённая промежуточная монета без налогов (решение 52).
        if i + 1 < hops.len() {
            let m = mids
                .iter()
                .find(|m| m.address == token_out)
                .ok_or_else(|| {
                    no(format!(
                        "промежуточная монета {token_out} сейчас не включена"
                    ))
                })?;
            v.mid_ok(m).map_err(no)?;
        }
        if v.taxed() && e.pool.kind != PoolKind::V2 {
            return Err(no("у монеты налог — через промежуточную только V2".into()));
        }
        let hs = graph_legs::hop_side(&e, token_in)
            .ok_or_else(|| no("ребро не соединяет токены хопа".into()))?;
        let usable = select::usable(q, &e, hs, v.usdt_dec)
            .map_err(|why| no(format!("{}: {why}", e.family.title)))?;
        let dec_in = q.tokens.decimals(ctx, token_in).await?;
        let dec_out = q.tokens.decimals(ctx, token_out).await?;
        legs.push(Leg {
            entry: e,
            usable,
            token_in,
            token_out,
            dec_in,
            dec_out,
        });
        running = token_out;
    }
    if running != end {
        return Err(no(
            "пулы доли не ведут к токену выхода сделки — цепочка не сходится".into(),
        ));
    }
    let tax = if v.side == Side::Buy {
        v.taxes.buy_pct
    } else {
        v.taxes.sell_pct
    };
    route_calc::eval(ctx, &legs, v.side, qty, route_calc::keep(tax))
}
