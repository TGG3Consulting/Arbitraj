//! Подлинность расчётного ERC20 проверяется один раз по финализированному коду и неизменяемой связи с мостом.
use super::Quoting;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    settings::cycle_cash::{self, Profile},
};
use alloy_primitives::Address;
pub(super) fn configured(q: &Quoting) -> GatewayResult<Option<Profile>> {
    cycle_cash::read(&q.ctx.settings, q.ctx.place.id)
        .map_err(|why| q.ctx.err(ErrorCode::NotConfigured, why))
}
pub(super) async fn checked(q: &Quoting, base: Address) -> GatewayResult<Option<Address>> {
    let ctx = &q.ctx;
    let Some(p) = configured(q)?.filter(|p| p.token == base) else {
        return Ok(None);
    };
    let mut entry = q.tokens.cycle_cash.lock().await;
    let revision = ctx.settings.generation();
    if *entry == Some((revision, p)) {
        return Ok(Some(p.token));
    }
    super::super::pools::cycle_cash::verify(ctx, p).await?;
    if revision != ctx.settings.generation() {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки расчётной валюты изменились во время проверки",
        ));
    }
    *entry = Some((revision, p));
    Ok(Some(p.token))
}

/// Сколько подтверждённых USDT нужно для восполнения именно потраченного количества ETH.
/// Стоимость по точному выходу учитывает комиссию и ликвидность каждой ноги обмена.
pub(super) fn replacement(
    q: &Quoting,
    physical: Address,
    cash: Address,
    wei: alloy_primitives::U256,
) -> GatewayResult<rust_decimal::Decimal> {
    use super::super::{num, pools::registry::Entry};
    use super::{graph_legs::hop_side, pool_quote, select};
    use alloy_primitives::U256;
    use rust_decimal::Decimal;
    if wei.is_zero() {
        return Ok(Decimal::ZERO);
    }
    let ctx = &q.ctx;
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let ask = |e: &Entry, input: Address, want: U256| -> Option<U256> {
        if !e.valid {
            return None;
        }
        let usable = select::usable(q, e, hop_side(e, input)?, 6).ok()?;
        if !usable.hook_pct.is_zero() || !usable.tax_pct.is_zero() {
            return None;
        }
        let fill = pool_quote::swap(ctx, e, input, false, want).ok()?;
        (fill.complete && fill.amount_out >= want && !fill.amount_in.is_zero())
            .then_some(fill.amount_in)
    };
    let direct = q
        .reg
        .pair_arcs(physical, wrapped)
        .iter()
        .filter_map(|e| ask(e, physical, wei))
        .min();
    let via = if cash != physical && cash != wrapped {
        let first = q.reg.pair_arcs(physical, cash);
        q.reg
            .pair_arcs(cash, wrapped)
            .iter()
            .filter_map(|last| {
                let need = ask(last, cash, wei)?;
                first
                    .iter()
                    .filter(|e| e.liquidity_site() != last.liquidity_site())
                    .filter_map(|e| ask(e, physical, need))
                    .min()
            })
            .min()
    } else {
        None
    };
    direct
        .into_iter()
        .chain(via)
        .min()
        .and_then(|n| num::to_decimal(n, 6))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "Нет проверенной котировки покупки потраченного ETH за расчётный USDT",
            )
        })
}

/// Начальный ориентир для ранжирования: исполнимая покупка 0,001 ETH, приведённая к одному ETH.
pub(super) fn native_usdt(
    q: &Quoting,
    physical: Address,
    cash: Address,
) -> GatewayResult<rust_decimal::Decimal> {
    let cost = replacement(
        q,
        physical,
        cash,
        alloy_primitives::U256::from(1_000_000_000_000_000u64),
    )?;
    Ok(cost * rust_decimal::Decimal::from(1000))
}
