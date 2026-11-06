//! Файл: venues/gate/wallet_cost.rs
//! Что делает: стоимость вывода с Gate — фиксированная комиссия сети + процентная доля × сумма (решение 199) из
//! справочника сетей (wallet.rs).
//! Кто вызывает: mod.rs (withdraw_cost из контракта).
//! От чего зависит: wallet.rs (справочник за Arc), ctx.rs, core/types.
//! Настройки: нет своих (через wallet.rs — venue.bsc_network, venue.networks_fresh_ms).
//!
//! Официальная документация: GET /wallet/withdraw_status — withdraw_fix_on_chains («Fixed withdrawal fee on multiple
//! chains») https://www.gate.com/docs/developers/apiv4/en/ ; withdraw_percent_on_chains «X%» — доля X/100, без «%» —
//! комиссия неизвестна (networks_parse.rs).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Cost, CostKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::wallet::{Networks, shared};

/// Стоимость вывода: фиксированная комиссия сети (withdraw_fix_on_chains) + доля × сумма (CoinNetworkInfo::fee_for)
/// в монетах. Принимает: монету, сеть, сумму вывода. Ошибки: монеты или сети нет, комиссия не сообщена или неясна, связь.
pub async fn withdraw_cost(
    ctx: &Ctx,
    nets: &Networks,
    symbols: &Symbols,
    asset: &Asset,
    network: &str,
    amount: Decimal,
) -> GatewayResult<Vec<Cost>> {
    let all = shared(ctx, nets, symbols).await?;
    let info = all
        .iter()
        .find(|c| c.asset.ticker == asset.ticker && c.network == network)
        .ok_or_else(|| {
            let m = format!("У {} нет сети {network} в справочнике Gate", asset.ticker);
            ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, m))
        })?;
    let fee = info.fee_for(amount).ok_or_else(|| {
        let m = format!(
            "Gate не сообщил комиссию вывода {} в сети {network} (или процентная без «%» — формат не описан)",
            asset.ticker
        );
        ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, m))
    })?;
    Ok(vec![Cost::new(
        CostKind::WithdrawFee,
        fee,
        asset.ticker.clone(),
        false,
    )])
}
