//! Нативная расчётная валюта EtherVista и покрытие расходов на каноническом блоке кошелька.
//! Содержит проверку терминального платежа при подборе и отправке.
use super::{Quoting, vista_pricing::is_vista, white_state::Head};
use crate::{
    core::types::RouteMode,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::Address;

pub(in crate::venues::bsc_dex) use super::super::exec::native::cash;
/// Валюта расчёта определяется настройкой площадки, в том числе у токенов без прямого пула EtherVista.
pub(in crate::venues::bsc_dex) fn cash_market(p: &super::super::parts::Parts)->bool {
    let Ok(ticker)=p.ctx.text("venue.quote_asset") else {return false};
    super::super::exec::native::cash_ticker(&p.ctx,&ticker)
}
/// Подтверждённый механизм перевода именно нативной расчётной валюты на одной или другой стороне сделки.
pub(in crate::venues::bsc_dex) fn terminal_cash(
    reg: &super::super::pools::registry::Registry,
    route: Option<&crate::core::types::DexRoute>,
) -> bool {
    let Some(r) = route else { return false };
    let [leg] = r.legs.as_slice() else {
        return false;
    };
    let [h] = leg.hops.as_slice() else {
        return false;
    };
    matches!(r.mode, RouteMode::Direct)
        && matches!(
            h.fee_mode,
            Some(crate::core::types::RouterFeeMode::NativeSettlement { .. })
        )
        && reg.entry(&h.pool).is_some_and(|e| is_vista(&e))
}
/// Проверяет аванс ETH, разрешения, газ и публикацию на балансе плательщика перед допуском.
pub(super) async fn funded(
    q: &Quoting,
    head: Head,
    payer: Address,
    value: alloy_primitives::U256,
    fill: &super::v2_wallet_sim::Fill,
    floor: alloy_primitives::U256,
) -> GatewayResult<()> {
    use super::super::{
        exec::gas_fund,
        node::rpc,
        send::{self, tx, tx_price},
    };
    use alloy_primitives::{U64, U256};
    let ctx = &q.ctx;
    let (fees, base) = q.gas.fees(ctx, &q.heads).await?;
    let price = tx_price::floored(fees, floor, base).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "Недоступна предельная цена газа EtherVista",
        )
    })?;
    let margin = ctx.dec("send.gas_limit_margin_pct")?;
    let tip_cost = if let Some(t) = send::planned_tip(ctx) {
        let units = send::tip_gas::planned_units(ctx).await?.ok_or_else(|| {
            ctx.err(
                ErrorCode::StaleData,
                "Газ перевода чаевых EtherVista неизвестен",
            )
        })?;
        Some((t, tx::with_margin(ctx, U64::from(units), margin)?))
    } else {
        None
    };
    let swap = tx::with_margin(ctx, U64::from(fill.swap_gas), margin)?;
    let approvals = tx::with_margin(ctx, U64::from(fill.approval.0), margin)?;
    let total = swap
        .saturating_add(approvals)
        .saturating_add(fill.approval.1.saturating_sub(1) as u64);
    let gas = total.saturating_add(tip_cost.map_or(0, |(_, g)| g));
    let extra = q.gas.extra(
        ctx,
        gas,
        1 + fill.approval.1 + usize::from(tip_cost.is_some()),
    )?;
    let need =
        gas_fund::need_wei(value, U256::from(price.max_fee), total, tip_cost).saturating_add(extra);
    let need = gas_fund::add_native_reserve(ctx, value, need)?;
    let available: U256 = rpc::call(
        ctx,
        "eth_getBalance",
        serde_json::json!([payer, head.selector()]),
    )
    .await?;
    if available < need {
        return Err(ctx.err(
            ErrorCode::InsufficientBalance,
            "EtherVista: ETH кошелька не покрывает аванс, резерв, разрешения и газ",
        ));
    }
    Ok(())
}
