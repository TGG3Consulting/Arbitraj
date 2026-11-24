//! Допуск нового входа между спотом и DEX до появления подтверждённого преобразования
//! единиц для конкретной биржи. Исполнение открытых позиций и атомарные циклы этот путь не вызывают.

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;

use super::engine::Core;

/// Проверка при открытии, одновременно с чтением сетей и балансов. Подбор объёма узел не опрашивает.
pub(super) async fn check_entry(core: &Core, d: &Direction) -> Result<(), String> {
    let buy = super::trade_ops::kind(core, &d.buy_on)?;
    let sell = super::trade_ops::kind(core, &d.sell_on)?;
    let dex = match (buy, sell) {
        (VenueKind::Cex, VenueKind::Dex) => &d.sell_on,
        (VenueKind::Dex, VenueKind::Cex) => &d.buy_on,
        _ => return Ok(()),
    };
    if super::native::is_native(core, &d.asset) {
        return Ok(());
    }
    let token = d.asset.contract.ok_or_else(|| {
        format!(
            "{}: нет адреса для проверки единицы перевода",
            d.asset.ticker
        )
    })?;
    let venue = core.inner.deps.gw.get(dex).await.map_err(|e| e.message)?;
    let scale = venue.scaled_token_unit(token).await.map_err(|e| {
        format!(
            "{} на {}: не подтверждена единица токена: {}",
            token, dex.0, e.message
        )
    })?;
    if let Some(m) = scale
        && m != Decimal::ONE
    {
        return Err(format!(
            "{} → {}, {} {token}: коэффициент контракта {m}; пересчёт между единицами этих площадок не подтверждён для входа",
            d.buy_on.0, d.sell_on.0, d.asset.ticker
        ));
    }
    Ok(())
}
