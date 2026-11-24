//! Проверка единиц одного завершённого вывода перед записью его недостачи.
use crate::core::settlement_unit::UnitRatio;
use crate::core::{
    engine::Core,
    types::{Asset, CostKind, Transfer, TransferStatus, VenueId},
    withdraw_seen::Seen,
};
use crate::gateway::contract::{Venue, VenueKind};
use rust_decimal::Decimal;

/// Стабильность проверяется для единиц обеих площадок, а не только их отношения.
pub(super) fn stable(pinned: Option<UnitRatio>, current: Option<UnitRatio>) -> Option<UnitRatio> {
    pinned.filter(|r| !r.is_identity() && Some(*r) == current)
}

/// Статусы с обеих сторон и именно тот экземпляр DEX, который получил этот перевод.
pub struct CreditProof<'a> {
    pub accepted_id: &'a str,
    pub sender: &'a Transfer,
    pub receiver: &'a Transfer,
    pub wallet: &'a dyn Venue,
}

fn same_credit<'a>(
    asset: &Asset,
    net: &str,
    accepted_id: &str,
    request: Decimal,
    sent: &'a Transfer,
    recv: &Transfer,
) -> Option<&'a str> {
    let hash = sent.tx_hash.as_deref()?;
    let got = recv.tx_hash.as_deref()?;
    if sent.status != TransferStatus::Sent
        || recv.status != TransferStatus::Credited
        || asset.contract.is_none()
        || sent.asset != *asset
        || recv.asset != *asset
        || sent.network != net
        || accepted_id.is_empty()
        || sent.venue_ref != accepted_id
        || sent.amount != request.trunc_with_scale(sent.amount.scale())
        || hash.is_empty()
        || crate::core::credit_watch::norm(hash) != crate::core::credit_watch::norm(got)
    {
        return None;
    }
    Some(hash)
}

impl<'a> CreditProof<'a> {
    /// Оба адаптера отнесли состоявшийся перевод к переданному отправителю ID,
    /// одной транзакции и одному активу. У части бирж в записи депозита есть
    /// только локальное имя: сравниваем его с тикером этого контракта у этой биржи.
    pub(super) fn matched_tx(
        &self,
        core: &Core,
        from: &VenueId,
        to: &VenueId,
        asset: &Asset,
    ) -> Option<&'a str> {
        let s = self.sender;
        let r = self.receiver;
        let identity = |record: &Transfer| {
            if record.asset.contract.is_some() {
                return record.asset == *asset;
            }
            let local = crate::core::venue_ticker::on_venue(core, asset, &record.venue);
            record.asset.ticker == local.ticker
                && record
                    .asset
                    .chain
                    .as_ref()
                    .is_none_or(|c| Some(c) == asset.chain.as_ref())
        };
        let h = s.tx_hash.as_deref()?;
        let got = r.tx_hash.as_deref()?;
        (s.venue == *from
            && r.venue == *to
            && self.wallet.id() == *to
            && s.status == TransferStatus::Sent
            && r.status == TransferStatus::Credited
            && !self.accepted_id.is_empty()
            && s.venue_ref == self.accepted_id
            && identity(s)
            && identity(r)
            && !h.is_empty()
            && crate::core::credit_watch::norm(h) == crate::core::credit_watch::norm(got))
        .then_some(h)
    }

    /// На обычном зачислении вызова к узлу нет. Остаточная недостача даже при наличии
    /// TransferWithUIAmount сохраняет прежнюю защиту: конкретная операция её не объясняет.
    pub(super) async fn in_sender_units(
        &self,
        core: &Core,
        to: &VenueId,
        net: &str,
        seen: &Seen,
        expected: Decimal,
        fee: Decimal,
    ) -> Option<(&'a str, Decimal, Decimal)> {
        let s = self.sender;
        let r = self.receiver;
        if self.wallet.kind() != VenueKind::Dex
            || self.wallet.id() != *to
            || crate::core::trade_ops::kind(core, &seen.from).ok()? != VenueKind::Cex
            || s.venue != seen.from
            || r.venue != *to
            || r.amount != seen.credited
        {
            return None;
        }
        let actual = s.fee.as_ref()?;
        if actual.kind != CostKind::WithdrawFee
            || actual.paid_in.is_some()
            || actual.in_price
            || actual.amount != fee
            || actual.asset != s.asset.ticker
        {
            return None;
        }
        let hash = same_credit(&seen.asset, net, self.accepted_id, seen.sent, s, r)?;
        let precision = crate::core::dirctx_venue::network(core, &seen.from, &seen.asset, net)
            .and_then(|n| n.withdraw_precision);
        if s.amount - fee == r.amount
            || precision
                .filter(|p| *p <= 28)
                .is_some_and(|p| (expected - seen.credited).abs() < Decimal::new(1, p))
        {
            return None;
        }
        let ui = self
            .wallet
            .confirmed_ui_credit(&seen.asset, hash, r.amount)
            .await
            .ok()??;
        exact_units(s.amount, fee, r.amount, ui).then_some((hash, ui, s.amount))
    }
}

fn exact_units(reported_gross: Decimal, fee: Decimal, raw: Decimal, ui: Decimal) -> bool {
    ui != raw && raw > Decimal::ZERO && ui > Decimal::ZERO && ui == reported_gross - fee
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn both_recorded_withdrawals_use_the_reported_precision_and_require_the_whole_credit() {
        let rows: Vec<Value> = serde_json::from_str(include_str!(
            "../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json"
        ))
        .unwrap();
        let receipts: Vec<Value> = serde_json::from_str(include_str!(
            "../../tests/data/bsc_units_2026_11_24/deposit_receipts.json"
        ))
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.len(), receipts.len());
        for (row, receipt) in rows.iter().zip(&receipts) {
            let mut sent: Transfer = serde_json::from_value(row["sent"].clone()).unwrap();
            let recv: Transfer = serde_json::from_value(row["credited"].clone()).unwrap();
            let requested = row["withdraw_requested"].as_str().unwrap().parse().unwrap();
            // То, что нормализация по ID принятого вывода передаёт shortfall (её тест отдельно).
            sent.asset = serde_json::from_value(row["asset_submitted"].clone()).unwrap();
            sent.network = "BSC".into();
            let id = &sent.venue_ref;
            assert_ne!(requested, sent.amount);
            let hash = same_credit(&recv.asset, "BSC", id, requested, &sent, &recv).unwrap();
            assert_eq!(Some(hash), receipt["transactionHash"].as_str());
            let logs = receipt["logs"].as_array().unwrap();
            let event = logs
                .iter()
                .find(|l| {
                    l["topics"][0].as_str()
                        == Some(
                            "0x0226a2f5c1ae0e071aeec3d4ebafcefdc5c549be11f40ed27e76e802acccf374",
                        )
                })
                .unwrap();
            let data = event["data"].as_str().unwrap().strip_prefix("0x").unwrap();
            let word = |s| Decimal::from_i128_with_scale(i128::from_str_radix(s, 16).unwrap(), 18);
            let (raw, ui) = (word(&data[..64]), word(&data[64..]));
            let fee = sent.fee.as_ref().unwrap().amount;
            assert_eq!(raw, recv.amount);
            assert!(exact_units(sent.amount, fee, raw, ui));
            let scale = UnitRatio {
                buy_per_raw: Decimal::from(8),
                sell_per_raw: Decimal::ONE,
                raw_decimals: Some(18),
            };
            assert_eq!(
                stable(Some(scale), Some(scale))
                    .unwrap()
                    .to_sell(sent.amount - fee)
                    .unwrap(),
                recv.amount
            );
            assert_eq!(scale.to_buy(recv.amount).unwrap(), ui);
            assert_eq!(stable(Some(scale), Some(UnitRatio::IDENTITY)), None);
            assert_eq!(stable(Some(scale), None), None);
            // Даже минимальный недобор или удержание 1–5 % не могут пройти точное равенство.
            for pct in [1, 2, 5] {
                let partial = ui * Decimal::from(100 - pct) / Decimal::ONE_HUNDRED;
                assert!(!exact_units(sent.amount, fee, raw, partial));
                assert!(super::super::shortfall_pct(ui, partial).unwrap() > Decimal::ZERO);
            }
            assert!(same_credit(&recv.asset, "ETH", id, requested, &sent, &recv).is_none());
            assert!(same_credit(&recv.asset, "BSC", "", requested, &sent, &recv).is_none());
        }
    }

    #[test]
    fn recorded_wkc_deposits_still_have_a_real_two_percent_transfer_tax() {
        let wkc: Value = serde_json::from_str(include_str!(
            "../../tests/data/wkc_mexc_2026-11-18/journal.json"
        ))
        .unwrap();
        for deal in wkc["deals"].as_array().unwrap() {
            let gross: Decimal = deal["bought"].as_str().unwrap().parse().unwrap();
            let net: Decimal = deal["deposit_record"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                stable(Some(UnitRatio::IDENTITY), Some(UnitRatio::IDENTITY)),
                None
            );
            assert!(!exact_units(gross, Decimal::ZERO, net, net));
            assert_eq!(
                super::super::shortfall_pct(gross, net),
                Some(Decimal::from(2))
            );
        }
    }
}
