//! Для одного физического V2-маршрута финальная оценка газа и выплаты берётся
//! из симуляции кошелька. Повтор общего выбора сохраняет другие рынки и составные пути.
use super::super::pools::registry::Entry;
use super::{Quoting, routing};
use crate::{
    core::types::{PoolRef, Quote, RouteMode},
    gateway::{errors::GatewayResult, quote_stage},
};

tokio::task_local! {static SINGLE: Vec<PoolRef>;}

/// Один канонический блок для сравнения всех подходящих реализаций. Отсутствующая
/// разновидность не читает ключ плательщика, настройки или узел.
pub(super) async fn candidates(
    q: &Quoting,
    i: &routing::Input,
    native: bool,
) -> (Vec<super::v2_execution::OptionQuote>, bool) {
    use super::{
        delta_pricing, frax_pricing, oasis_pricing, router_pricing, white_pricing, white_state,
    };
    let mut kinds = [false; 14];
    for e in &i.entries {
        kinds[5] |= super::vista_pricing::is_vista(e) || (native && super::vista_pricing::native_router(e));
        kinds[10] |= super::super::exec::carbon_direct::enabled(e);
        kinds[9] |= super::super::exec::mint_club_direct::enabled(e);
        kinds[8] |= super::ekubo_v2_pricing::enabled(e);
        if native {
            continue;
        }
        kinds[13] |= e.family.variant.exact_router();
        kinds[12] |= e.family.variant.umbrae();
        kinds[11] |= super::super::pools::sectorone::enabled(e);
        kinds[7] |= super::module_pricing::enabled(e);
        kinds[6] |= super::super::exec::vault_direct::enabled(e);
        kinds[4] |= oasis_pricing::is_oasis(e);
        kinds[0] |= white_pricing::is_white(e);
        kinds[1] |= frax_pricing::is_frax(e);
        kinds[2] |= delta_pricing::is_delta(e);
        kinds[3] |= router_pricing::is_native(e);
    }
    if !kinds.into_iter().any(|v| v) {
        return (Vec::new(), false);
    }
    let head = if q.exec_quote {
        let Ok(h) = white_state::head(&q.ctx, "latest").await else {
            return (Vec::new(), true);
        };
        Some(h)
    } else {
        None
    };
    let (white, frax, delta, router, oasis, vista, vault, memebox, ekubo, mint_club, sector, umbrae, registered_v3, carbon) = tokio::join!(
        async {
            if kinds[0] {
                Box::pin(white_pricing::candidates(q, i, head)).await
            } else {
                Vec::new()
            }
        },
        async {
            if kinds[1] {
                Box::pin(frax_pricing::candidates(q, i, head)).await
            } else {
                Vec::new()
            }
        },
        async {
            if kinds[2] {
                Box::pin(delta_pricing::candidates(q, i, head)).await
            } else {
                Vec::new()
            }
        },
        async {
            if kinds[3] {
                Box::pin(router_pricing::candidates(q, i, head)).await
            } else {
                Vec::new()
            }
        },
        async {
            if kinds[4] {
                Box::pin(oasis_pricing::candidates(q, i, head)).await
            } else {
                Vec::new()
            }
        },
        async {
            if kinds[5] {
                Box::pin(super::vista_pricing::candidates(q, i, head)).await
            } else {
                Vec::new()
            }
        },
        async {
            if kinds[6] { Box::pin(super::vault_pricing::candidates(q,i,head)).await } else { Vec::new() }
        },
        async {
            if kinds[7] { Box::pin(super::module_pricing::candidates(q,i,head)).await } else { Vec::new() }
        },
        async { if kinds[8] { Box::pin(super::ekubo_v2_pricing::candidates(q,i,head)).await } else { Vec::new() } },
        async { if !native && kinds[9] { Box::pin(super::mint_club_pricing::candidates(q,i,head)).await } else { Vec::new() } },
        async { if kinds[11] {Box::pin(super::sectorone::candidates(q,i,head)).await} else {Vec::new()} },
        async { if kinds[12] {Box::pin(super::umbrae::candidates(q,i,head)).await} else {Vec::new()} },
        async { if kinds[13] {Box::pin(super::registered_v3_pricing::candidates(q,i,head)).await} else {Vec::new()} },
        async { if kinds[10] { Box::pin(super::carbon_pricing::candidates(q,i,head)).await }else{Vec::new()} }
    );
    let mut all = white;
    all.extend(memebox);
    all.extend(ekubo);
    all.extend(mint_club);
    all.extend(carbon);
    all.extend(sector);
    all.extend(umbrae);
    all.extend(registered_v3);
    all.extend(frax);
    all.extend(delta);
    all.extend(router);
    all.extend(oasis);
    all.extend(vista);
    all.extend(vault);
    (all, q.exec_quote)
}

fn individual(e: &Entry, native: bool) -> bool {
    if super::super::exec::mint_club_direct::enabled(e) || super::super::exec::carbon_direct::enabled(e) {return true;}
    if native && super::ekubo_v2_pricing::enabled(e) { return true; }
    if native {
        return super::vista_pricing::native_router(e);
    }
    super::ekubo_v2_pricing::enabled(e) || super::module_pricing::enabled(e) || super::super::exec::vault_direct::enabled(e) || super::vista_pricing::is_vista(e)
        || super::oasis_pricing::is_oasis(e)
        || super::white_pricing::is_white(e)
        || super::frax_pricing::is_frax(e)
        || super::router_pricing::is_native(e)
        || e.family.variant.exact_router() || e.family.variant.umbrae() || super::super::pools::sectorone::enabled(e)
        || super::delta_pricing::is_delta(e)
}

pub(super) fn excluded(e: &Entry) -> bool {
    SINGLE
            .try_with(|p| p.contains(&e.pool.reference))
            .unwrap_or(false)
}

fn one_leg(legs: &[crate::core::types::RouteLeg]) -> Option<&PoolRef> {
    let [leg] = legs else {
        return None;
    };
    let [hop] = leg.hops.as_slice() else {
        return None;
    };
    Some(&hop.pool)
}

pub(super) fn one(q: &Quote) -> Option<&PoolRef> {
    one_leg(&q.route.as_ref()?.legs)
}

/// Переносит признак mixed до отправки работы в CPU; на площадке без таких пулов
/// остаётся единственный обычный выбор без копии его входа.
pub(super) fn ordinary(
    q: Quoting,
    i: routing::Input,
    enabled: bool,
    native: bool,
) -> impl std::future::Future<Output = GatewayResult<Quote>> {
    let settlement = quote_stage::settlement();
    async move {
        if !enabled {
            return Box::pin(routing::allowed(q, i, settlement)).await;
        }
        let first = Box::pin(routing::allowed(q.clone(), i.clone(), settlement)).await?;
        let Some(pool) = one(&first) else {
            return Ok(first);
        };
        if !i
            .entries
            .iter()
            .any(|e| individual(e, native) && &e.pool.reference == pool)
        {
            return Ok(first);
        }
        let pools = i
            .entries
            .iter()
            .filter(|e| individual(e,native))
            .map(|e| e.pool.reference.clone())
            .collect::<Vec<_>>();
        // В графе одиночный пул мог занять место другой исполнимой площадки.
        // Для альтернативного расчёта составные пути сохраняются; общие кэши не меняются.
        let mut i = i;
        i.entries.retain(|e| !individual(e,native));
        SINGLE
            .scope(pools, Box::pin(routing::allowed(q, i, settlement)))
            .await
    }
}

/// При единственном вызове Router02 приватный бандл несёт те же swap/approve, но его
/// каналы и чаевые отличаются. Использует тот же результат моделирования кошелька.
pub(super) fn private(
    q: &Quoting,
    i: &routing::Input,
    options: &mut Vec<super::v2_execution::OptionQuote>,
) {
    use super::super::exec::executor;
    use super::{direct, graph_exec, path::Path};
    if quote_stage::settlement() || q.ctx.text("dex.router_mode").ok().as_deref() != Some("graph") {
        return;
    }
    let mut extra = Vec::new();
    for (quote, gas) in options.iter() {
        let Some(r) = quote.route.as_ref() else {
            continue;
        };
        if r.mode != RouteMode::Direct {
            continue;
        }
        let Some(e) = one(quote).and_then(|p| q.reg.get(p)) else {
            continue;
        };
        if !e.family.fork || !e.family.tradable || super::super::exec::mint_club_direct::enabled(&e) || super::super::exec::carbon_direct::enabled(&e) || e.family.variant.wallet_module() || super::ekubo_v2_pricing::enabled(&e) || super::oasis_pricing::is_oasis(&e) {
            continue;
        }
        let white = super::white_pricing::is_white(&e);
        let Ok((c, _)) = direct::best(q, &i.asset, i.side, i.qty, vec![e], i.coin_dec, i.usdt_dec)
        else {
            continue;
        };
        // Приватный конвейер сверяет публичный getAmountsOut WhiteSwap без WSE.
        // При разнице в стоимости оставить отдельную адресную сделку со скидкой.
        if white
            && (c.coins != quote.fillable_qty || c.usdt != quote.avg_price * quote.fillable_qty)
        {
            continue;
        }
        let Ok(path) = Path::one(&q.ctx, c, i.side, (i.coin_dec, i.usdt_dec)) else {
            continue;
        };
        if graph_exec::executable(&q.ctx, &[path], false).is_err() {
            continue;
        }
        let mut copy = quote.clone();
        let Some(r) = copy.route.as_mut() else {
            continue;
        };
        r.mode = RouteMode::Graph;
        if matches!(executor::takes(&q.ctx, &q.reg, r, false), Ok(None))
            && graph_exec::graph_form(&q.ctx, r) == graph_exec::GraphForm::Bundle
        {
            extra.push((copy, *gas));
        }
    }
    options.extend(extra);
}

#[cfg(test)]
mod tests {
    use super::one_leg;
    use crate::core::types::{PoolKind, PoolRef, RouteHop, RouteLeg};
    use rust_decimal::Decimal;
    use serde_json::Value;

    #[test]
    fn only_one_physical_swap_is_replaced_by_its_wallet_simulation() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/base_v2_factories.json"
        ))
        .unwrap();
        let id = "bakeryswap-base";
        let sample = fixtures[id]["native_router"]["samples"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["cases"].is_array())
            .unwrap();
        let pool = PoolRef::Contract(sample["pool"].as_str().unwrap().parse().unwrap());
        let hop = RouteHop {
            fee_mode: None,
            family: id.into(),
            kind: PoolKind::V2,
            pool: pool.clone(),
        };
        let leg = RouteLeg {
            share: Decimal::ONE,
            hops: vec![hop.clone()],
            expected_out: sample["cases"][0]["buy"]["received"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        };
        assert_eq!(one_leg(std::slice::from_ref(&leg)), Some(&pool));
        assert!(one_leg(&[leg.clone(), leg.clone()]).is_none());
        let mut multi = leg;
        multi.hops.push(hop);
        assert!(one_leg(&[multi]).is_none());
        assert!(one_leg(&[]).is_none());
    }
}
