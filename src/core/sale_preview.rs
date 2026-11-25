//! Read-only estimate before the existing UI/Telegram sale confirmation.
use rust_decimal::Decimal;

use super::{deal::Deal, engine::Core, manual_price, manual_price_exec, sale_price};
use crate::core::types::ManualAction;
use crate::notify::human;

impl ManualAction {
    pub fn sale_preview_record(&self) -> Option<(String, bool)> {
        match self {
            Self::SellNow { alert_id } => Some((format!("a.{alert_id}"), false)),
            Self::SellNowReturn { alert_id } => Some((format!("a.{alert_id}"), true)),
            Self::SellHalted { deal_id } => Some((format!("d.{deal_id}"), false)),
            Self::SellHaltedReturn { deal_id } => Some((format!("d.{deal_id}"), true)),
            _ => None,
        }
    }
}

impl Core {
    pub async fn manual_sale_preview(
        &self,
        key: &str,
        with_return: bool,
    ) -> Result<String, String> {
        let (_, original) = sale_price::record(self, key)?;
        let qty = manual_price_exec::quote_qty(self, &original).await?;
        let mut position = original.clone();
        position.qty = qty;
        let past = manual_price::previous_cash(self, &position)?;
        let wallet = manual_price_exec::wallet(self, &position, &position.at)?;
        let value = manual_price::scenario(
            self,
            &position,
            &position.at,
            wallet.as_ref(),
            &past,
            true,
            with_return,
        )
        .await?;
        if !manual_price_exec::matches(&original, &sale_price::record(self, key)?.1) {
            return Err("Состояние сделки изменилось. Обновите оценку.".into());
        }
        let prior_cash = past.iter().map(|(_, amount)| *amount).sum();
        let settlement = if with_return {
            "с возвратом на площадку покупки"
        } else {
            "до возможного возврата USDT"
        };
        Ok(format!(
            "Если продать {} {} здесь ({}) {}: {}. {}",
            qty.normalize(),
            position.direction.asset.ticker,
            position.at.0,
            settlement,
            human::signed_usd(value),
            pnl_text(&position, value, prior_cash)
        ))
    }
}

/// The displayed proceeds include prior sale cash net of its quote-currency fees.
/// The ledger already books those fills and fees. Add only incremental net cash.
pub(super) fn pnl_text(deal: &Deal, net: Decimal, previous_cash: Decimal) -> String {
    let remaining = net - previous_cash;
    let pnl = deal.result_if(remaining);
    let mut text = format!("Расчётный итог сделки: {}", human::signed_usd(pnl));
    if deal.invested > Decimal::ZERO {
        text.push_str(&format!(" ({:+.2}%)", -deal.loss_pct_if(remaining)));
    }
    if deal.group.as_ref().is_some_and(|g| !g.left) {
        text.push_str(
            ". Сумма относится к этой сделке; при общем возврате группы итог может измениться.",
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_sale_estimate_matches_closed_deal_without_double_counting() {
        let source: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/data/poloniex_manual_2026_11_25/statement.json"
        ))
        .unwrap();
        let number = |k: &str| source[k].as_str().unwrap().parse::<Decimal>().unwrap();
        let mut d = Deal::new(
            source["id"].as_str().unwrap().into(),
            serde_json::from_value(source["direction"].clone()).unwrap(),
            serde_json::Value::Null,
            Decimal::ZERO,
        );
        d.invested = number("invested");
        d.spent = source["costs_before_sale"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse::<Decimal>().unwrap())
            .sum();
        let net = number("gross") - number("sale_fee") - number("return_cost");
        let result = number("result");
        assert_eq!(d.result_if(net), result);
        let first = pnl_text(&d, net, Decimal::ZERO);
        assert!(first.contains("−28.05 USDT"));
        assert!(first.contains("-93.51%"));
        // Same ledger after accounting for part of the sale. The visible total still includes both fills.
        d.proceeds = number("gross") / Decimal::TWO;
        let paid_fee = number("sale_fee") / Decimal::TWO;
        d.spent += paid_fee;
        let prior_cash = d.proceeds - paid_fee;
        assert_eq!(pnl_text(&d, net, prior_cash), first);
        d.group = Some(super::super::deal_group::DealGroup::default());
        assert!(pnl_text(&d, net, prior_cash).contains("возврате группы"));
        d.group.as_mut().unwrap().left = true;
        assert_eq!(pnl_text(&d, net, prior_cash), first);
    }
}
