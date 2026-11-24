use super::*;
use crate::core::types::{Direction, TradeResult, Transfer};
use serde::Deserialize;

#[derive(Deserialize)]
struct Entry {
    direction: Direction,
    dex_account: DexAccountId,
    buy: TradeResult,
    relays: u64,
}

#[derive(Deserialize)]
struct TransferPair {
    sent: Transfer,
    credited: Transfer,
}

// Одна покупка Poloniex и её реальный вывод в BSC. Идентификаторы и время покупки
// обезличены. Денежные поля, названия монеты в обоих отчётах и масштаб сохранены.
#[test]
fn observed_matches_recorded_poloniex_fill_with_bsc_credit_after_fees() {
    let entry: Entry = serde_json::from_str(include_str!(
        "../../tests/data/bsc_units_2026_11_24/deal_entry.json"
    ))
    .unwrap();
    let ledger: Vec<TransferPair> = serde_json::from_str(include_str!(
        "../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json"
    ))
    .unwrap();
    let first = ledger.into_iter().next().unwrap();
    let dex = entry.direction.sell_on.clone();
    let mut deal = Deal::new(
        entry.buy.request.deal_id.clone(),
        entry.direction,
        serde_json::Value::Null,
        Decimal::ZERO,
    );
    deal.dex_account = Some(entry.dex_account);
    deal.relays = entry.relays;
    deal.buy = Some(entry.buy);
    deal.transfers = vec![first.sent, first.credited];

    let row = observed(&deal, &dex).expect("покупка и отчёты перевода должны совпасть");
    assert!(row.withdrawal);
    assert_eq!(row.cex, deal.direction.buy_on);
    assert_eq!(row.hash, deal.transfers[1].tx_hash.as_deref().unwrap());
    assert_eq!(row.exchange, "3.22869477".parse::<Decimal>().unwrap());
    assert_eq!(row.raw, "0.40358684625".parse::<Decimal>().unwrap());
    assert_eq!(row.exchange, row.raw * Decimal::from(8));

    // Без факта комиссии покупки записанная сумма вывода с этой покупкой не сходится.
    deal.buy.as_mut().unwrap().fees.clear();
    assert!(observed(&deal, &dex).is_none());
}
