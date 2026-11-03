//! Файл: venues/bsc_dex/exec/dry_size.rs
//! Что делает: сухая сделка DEX (решение 121, C4) — количество монет под сумму USDT: спот-цена пулов реестра (медиана
//! прямых пулов «монета — USDT», иначе через активные промежуточные монеты; quote/spot.rs) → котировка → уточнение
//! второй котировкой (количество = сумма / средняя цена первой); пулы маршрута для итога.
//! Кто вызывает: exec/dry_swap.rs.
//! От чего зависит: quote/mod.rs (Quoting::quote), quote/spot.rs, pools/registry.rs, tokens.rs, num.rs.
//! Настройки: через вызываемые — как у котировки.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{DexDrySwap, DexRoute, DrySwapPool, Quote};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::quote::{Quoting, spot};

/// Количество монет под сумму USDT: спот-цена пулов реестра (медиана) → котировка → уточнение второй котировкой
/// (количество = сумма / средняя цена первой). Возвращает: вторую котировку; qty — в итог. Ошибки: NoRoute (нет пула
/// для спот-цены), котировка.
pub async fn sized(q: &Quoting, r: &mut DexDrySwap) -> GatewayResult<Quote> {
    let ctx = &q.ctx;
    let coin = r.asset.bsc_contract.ok_or_else(|| {
        let why = format!("{}: нет адреса контракта в BSC", r.asset.ticker);
        ctx.err(ErrorCode::NoRoute, why)
    })?;
    let usdt = q.tokens.usdt(ctx)?;
    let (cd, ud) = (
        q.tokens.decimals(ctx, coin).await?,
        q.tokens.decimals(ctx, usdt).await?,
    );
    let bad = || {
        ctx.err(
            ErrorCode::BadResponse,
            "сумма не переводится в единицы токена",
        )
    };
    let units = num::to_units(r.usdt, ud).ok_or_else(bad)?;
    let spot = spot_coins(q, coin, usdt, units).ok_or_else(|| {
        let why = "нет прочитанного пула монета–USDT (или через промежуточную) для спот-цены — количество не подобрать";
        ctx.err(ErrorCode::NoRoute, why)
    })?;
    let qty0 = num::to_decimal(spot, cd).ok_or_else(bad)?;
    let first = q.quote(&r.asset, r.side, qty0).await?;
    if first.avg_price <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "средняя цена котировки 0"));
    }
    let qty = (r.usdt / first.avg_price).round_dp(u32::from(cd));
    let quote = q.quote(&r.asset, r.side, qty).await?;
    if quote.fillable_qty < quote.qty {
        let note = format!(
            "ликвидности хватает на {} из {} монет — собрано на {}",
            quote.fillable_qty, quote.qty, quote.fillable_qty
        );
        r.notes.push(note);
    }
    r.qty = Some(quote.fillable_qty);
    Ok(quote)
}

/// Монет за сумму USDT по текущей цене пулов реестра: прямые пулы «монета — USDT», иначе через активные промежуточные
/// монеты (USDT → промежуточная → монета). Медиана по пулам (мусорный пул с чужой ценой не задаёт размер). None — нет
/// прочитанного пула. Ошибок нет.
fn spot_coins(q: &Quoting, coin: Address, usdt: Address, units: U256) -> Option<U256> {
    let mut list: Vec<U256> = q
        .reg
        .by_pair(coin, usdt)
        .iter()
        .filter(|e| e.valid)
        .filter_map(|e| spot::coins_for(e, units))
        .collect();
    if list.is_empty() {
        for m in q.mids.active(&q.ctx) {
            for a in q.reg.by_pair(m.address, usdt).iter().filter(|e| e.valid) {
                let Some(mid) = spot::coins_for(a, units) else {
                    continue;
                };
                let pools = q.reg.by_pair(coin, m.address);
                list.extend(
                    pools
                        .iter()
                        .filter(|e| e.valid)
                        .filter_map(|e| spot::coins_for(e, mid)),
                );
            }
        }
    }
    list.sort();
    list.get(list.len() / 2).copied().filter(|v| !v.is_zero())
}

/// Пулы маршрута по долям. Ошибок нет.
pub fn pools(route: &DexRoute) -> Vec<DrySwapPool> {
    route
        .legs
        .iter()
        .enumerate()
        .flat_map(|(i, l)| {
            l.hops.iter().map(move |h| DrySwapPool {
                dex: h.family.clone(),
                kind: h.kind,
                pool: h.pool.clone(),
                leg: i,
            })
        })
        .collect()
}
