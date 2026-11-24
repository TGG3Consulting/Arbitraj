//! Единица сети из Binance Wallet /sapi/v1/capital/config/getall.
//! Binance определяет denomination как число токенов сети на одну биржевую единицу.
//! https://developers.binance.com/docs/wallet/capital/all-coins-info

use std::collections::HashMap;

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde_json::Value;

use super::num;
use crate::core::types::{Asset, CoinNetworkInfo};

/// Поле имеет тип int64. Не округляем неделимый коэффициент под ёмкость Decimal.
pub(super) fn per_raw(row: &Value) -> Result<Option<Decimal>, String> {
    if row.get("denomination").is_none_or(Value::is_null) {
        return Ok(None);
    }
    let lot = num::int(row, "denomination").map_err(|e| e.message)?;
    if lot == 1 {
        return Ok(None);
    }
    let ratio = Decimal::ONE
        .checked_div(Decimal::from(lot))
        .filter(|v| {
            lot > 0 && v.mantissa().checked_mul(i128::from(lot)) == Some(10_i128.pow(v.scale()))
        })
        .ok_or_else(|| {
            format!("denomination {lot}: нет точного положительного обратного коэффициента")
        })?;
    Ok(Some(ratio.normalize()))
}

struct Row {
    contract: Address,
    ratio: Option<Decimal>,
    withdraw: bool,
    deposit: bool,
}

/// Своя сеть → биржевой тикер → её единственная строка. None — неоднозначная или неполная запись.
#[derive(Default)]
pub(super) struct Units(HashMap<String, HashMap<String, Option<Row>>>);

impl Units {
    pub(super) fn record(
        &mut self,
        network: &str,
        ticker: &str,
        parsed: Option<(&CoinNetworkInfo, Option<Decimal>)>,
    ) {
        let row = parsed.and_then(|(i, ratio)| {
            Some(Row {
                contract: i.asset.contract?,
                withdraw: i.withdraw_enabled,
                deposit: i.deposit_enabled,
                ratio,
            })
        });
        self.0
            .entry(network.to_owned())
            .or_default()
            .entry(ticker.to_owned())
            .and_modify(|v| *v = None)
            .or_insert(row);
    }

    pub(super) fn for_side(
        &self,
        asset: &Asset,
        network: &str,
        withdrawing: bool,
    ) -> Result<Option<Decimal>, String> {
        let row = self
            .0
            .get(network)
            .and_then(|m| m.get(&asset.ticker))
            .and_then(Option::as_ref)
            .filter(|r| Some(r.contract) == asset.contract);
        let r = row.ok_or_else(|| {
            format!(
                "Binance {}/{}: единица не подтверждена справочником для этого контракта",
                asset.ticker, network
            )
        })?;
        let enabled = if withdrawing { r.withdraw } else { r.deposit };
        if !enabled {
            return Err(format!(
                "Binance {}/{}: {} закрыт",
                asset.ticker,
                network,
                if withdrawing {
                    "вывод"
                } else {
                    "ввод"
                }
            ));
        }
        Ok(r.ratio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VenueId;
    use crate::venues::binance::networks_parse::{DepositMin, parse_networks};

    #[test]
    fn documented_wallet_lot_binds_both_directions_to_one_market_and_network() {
        let source = include_str!("../../../tests/data/binance_wallet_denomination/getall.json");
        let json: Value = serde_json::from_str(source).unwrap();
        let parsed = parse_networks(
            &json,
            "BSC",
            &VenueId("binance".into()),
            DepositMin::default(),
        )
        .unwrap();
        assert!(parsed.skipped.is_empty());
        assert_eq!(parsed.list.len(), 1);
        let info = &parsed.list[0];
        for withdrawing in [true, false] {
            assert_eq!(
                parsed.units.for_side(&info.asset, "BSC", withdrawing),
                Ok(Some(Decimal::new(1, 6)))
            );
            assert!(
                parsed
                    .units
                    .for_side(&info.asset, "ETH", withdrawing)
                    .is_err()
            );
            let mut other = info.asset.clone();
            other.contract = None;
            assert!(parsed.units.for_side(&other, "BSC", withdrawing).is_err());
        }
        assert_eq!(info.withdraw_fee, Some(Decimal::from(10)));
        assert_eq!(info.withdraw_step, Some(Decimal::new(1, 2)));
        let mut repeated = Units::default();
        for _ in 0..2 {
            repeated.record(
                "BSC",
                &info.asset.ticker,
                Some((info, Some(Decimal::new(1, 6)))),
            );
        }
        assert!(repeated.for_side(&info.asset, "BSC", true).is_err());
    }
}
