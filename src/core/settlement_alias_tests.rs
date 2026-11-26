//! Файл: core/settlement_alias_tests.rs.
//! Что делает: сверяет разные тикеры одного ERC-20 на фактическом выводе Poloniex и в публичном справочнике.
//! Кто вызывает: GitHub Actions.
//! От чего зависит: записи SNDK/SNDKB одной операции, сетевой ответ биржи, сериализация рынка.
//! Настройки: сеть исходной операции.
use super::{Market, cash_alias};
use crate::core::{
    settlement_network::unique,
    types::{Asset, ChainId, VenueId},
};
use serde_json::Value;

#[test]
fn verified_poloniex_ticker_keys_the_cash_order_subscription_and_fee() {
    let archive: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json"
    ))
    .unwrap();
    let raw: Value = serde_json::from_str(include_str!(
        "../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_currency.json"
    ))
    .unwrap();
    let row = &archive[0];
    let dex: Asset = serde_json::from_value(row["credited"]["asset"].clone()).unwrap();
    let submitted: Asset = serde_json::from_value(row["asset_submitted"].clone()).unwrap();
    let cex = VenueId(row["sent"]["venue"].as_str().unwrap().into());
    let net = raw["body"]["networkList"][0]["blockchain"]
        .as_str()
        .unwrap();
    let rows = crate::venues::poloniex::networks_parse::parse(&raw["body"], net, &cex, true)
        .unwrap()
        .list;
    let wire = unique(&rows, &cex, net, &dex).unwrap();
    assert_eq!(wire.asset.ticker, submitted.ticker);
    assert_ne!(wire.asset.ticker, dex.ticker);
    assert_eq!(wire.asset.contract, dex.contract);
    let m = Market {
        evm_chain_id: 56,
        chain: ChainId("bsc".into()),
        cex_network: net.into(),
        dex: VenueId(row["credited"]["venue"].as_str().unwrap().into()),
        cex,
        quote: "USDT".into(),
        cex_asset_symbol: cash_alias(&dex, &wire.asset),
        asset: dex,
    };
    assert_eq!(m.book_key(), (m.cex.clone(), submitted.ticker.clone()));
    assert!(m.matches(&submitted));
    assert_eq!(m.cash_on(&m.cex).ticker, submitted.ticker);
    assert_eq!(m.cash_on(&m.dex).ticker, "SNDKB");
    let restored: Market = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert!(m.identical(&restored));
    let mut old = serde_json::to_value(&m).unwrap();
    old.as_object_mut().unwrap().remove("cex_asset_symbol");
    let old: Market = serde_json::from_value(old).unwrap();
    assert!(!m.identical(&old));
    assert!(!old.matches(&submitted));
    assert!(
        serde_json::to_value(&old)
            .unwrap()
            .get("cex_asset_symbol")
            .is_none()
    );
    let duplicated = [wire.clone(), wire.clone()];
    assert!(unique(&duplicated, &m.cex, net, &m.asset).is_err());
    assert!(unique(&rows, &m.cex, "", &m.asset).is_err());
}
