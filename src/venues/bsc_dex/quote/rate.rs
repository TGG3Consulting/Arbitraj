//! Файл: venues/bsc_dex/quote/rate.rs
//! Что делает: пересчёт сумм между USDT и встречным токеном пула (промежуточной монетой, решение 52) по живому
//! состоянию лучшего достоверного пула «промежуточная — USDT» реестра — локальной математикой пула (та же, что
//! у котировки). Нужен там, где сумма задана в USDT, а пул — с промежуточной монетой: сумма пробы
//! (check.probe_usdt) и ликвидность пула против dex.min_pool_liquidity_usd.
//! Кто вызывает: check/taxes.rs, check/hooks_probe.rs, check/quoter_cycle.rs, quote/select.rs; best_in — quote/route_gas.rs,
//! pools/ladder_counter.rs (решение 306: суммы лесенки цен пула во встречном токене).
//! От чего зависит: pools/registry.rs, quote/pool_quote.rs, ctx.rs.
//! Настройки: через pool_quote — dex.v2_fee_pct [вид пула V2].

use std::borrow::Borrow;

use alloy_primitives::{Address, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::pools::registry::{Entry, Registry};
use super::pool_quote;

/// Сколько единиц встречного токена `quote` дают `amount` единиц USDT (точный вход, лучший пул quote–USDT).
/// quote = USDT — та же сумма. Ошибки: NoRoute — нет достоверного пула «quote — USDT», расчёт невозможен.
pub fn from_usdt(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    quote: Address,
    amount: U256,
) -> GatewayResult<U256> {
    best(ctx, reg, usdt, quote, usdt, amount)
}

/// Сколько единиц USDT дают `amount` единиц встречного токена `quote` (точный вход, лучший пул quote–USDT).
/// quote = USDT — та же сумма. Ошибки: NoRoute — нет достоверного пула «quote — USDT», расчёт невозможен.
pub fn to_usdt(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    quote: Address,
    amount: U256,
) -> GatewayResult<U256> {
    best(ctx, reg, usdt, quote, quote, amount)
}

/// Наибольший выход по пулам «quote — USDT» на точный вход token_in. Ошибки: NoRoute.
fn best(
    ctx: &Ctx,
    reg: &Registry,
    usdt: Address,
    quote: Address,
    token_in: Address,
    amount: U256,
) -> GatewayResult<U256> {
    if quote == usdt {
        return Ok(amount);
    }
    // Решение 410 (t195): пулы пары без копии записей (Arc) — раньше by_pair копировал каждую запись (Pool, строки,
    // состояние) на каждый пересчёт суммы; читается только состояние.
    let direct=best_in(ctx,&reg.pair_arcs(quote,usdt),quote,token_in,amount);
    let Some(w)=wrapped_bridge(ctx,usdt,quote) else{return direct;};
    let first=bridge_pools(reg,token_in,w);
    let target=if token_in==quote {usdt} else {quote};
    let bridge=best_in(ctx,&first,w,token_in,amount).and_then(|mid|
        best_in(ctx,&bridge_pools(reg,w,target),target,w,mid));
    match (direct,bridge) {
        (Ok(a),Ok(b))=>Ok(a.max(b)),
        (Err(_),Ok(b))=>Ok(b),
        (a,_)=>a
    }
}

/// Две разрешённые промежуточные валюты и отдельное разрешение искать их общую пару.
pub(crate) fn wrapped_bridge(ctx:&Ctx,usdt:Address,quote:Address)->Option<Address>{
    let (w,allowed)=bridge_settings(ctx)?;
    (w!=quote && w!=usdt && quote!=usdt && allowed.contains(&quote)).then_some(w)
}
pub(crate) fn bridge_settings(ctx:&Ctx)->Option<(Address,Vec<Address>)>{
    if !ctx.flag("dex.intermediate_wrapped_bridge").unwrap_or(false) || !ctx.flag("dex.intermediate_enabled").unwrap_or(false) ||
        ctx.text("dex.router_mode").ok().as_deref()!=Some("graph") || ctx.int("dex.route_max_hops").ok()?<3 {return None;}
    let w=ctx.addr("dex.native_wrapped").ok()?;
    let allowed=ctx.settings.addresses("dex.intermediate_tokens",ctx.at()).ok()?;
    allowed.contains(&w).then_some((w,allowed))
}
pub(crate) fn bridge_pools(reg:&Registry,a:Address,b:Address)->Vec<std::sync::Arc<Entry>>{
    let mut pairs=reg.pair_arcs(a,b);
    pairs.extend(reg.pair_arcs(b,a));
    pairs
}

/// То же по уже взятым пулам «quote — USDT» (решение 129: учёт газа в графе берёт их один раз на котировку, а не на
/// каждый путь). Принимает: пулы пары (копии записей или `Arc` записей реестра — решение 410, t195), встречный токен (для
/// текста), токен входа, сумму. Ошибки: NoRoute.
pub fn best_in<E: Borrow<Entry>>(
    ctx: &Ctx,
    pools: &[E],
    quote: Address,
    token_in: Address,
    amount: U256,
) -> GatewayResult<U256> {
    pools
        .iter()
        .map(<E as Borrow<Entry>>::borrow)
        .filter(|e| e.valid)
        .filter_map(|e| pool_quote::swap(ctx, e, token_in, true, amount).ok())
        .filter(|s| s.complete)
        .map(|s| s.amount_out)
        .max()
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("нет достоверного пула {quote} — USDT для пересчёта суммы"),
            )
        })
}
