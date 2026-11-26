//! Файл: core/settlement_target_tests.rs.
//! Что делает: повторяет отказ между записью номера транзакции и получением результата используя реальные ID Base.
//! Кто вызывает: GitHub Actions.
//! От чего зависит: две публичные квитанции и рабочий SQLite-журнал исполнителя.
//! Настройки: отсутствуют, сетевые запросы не требуются.
use super::{Market, key, log};
use crate::{
    core::types::{Asset, ChainId, DexAccountId, Side, TradeRequest, VenueId},
    gateway::pending::PendingRef,
    storage::db::Db,
    venues::bsc_dex::{exec::settlement_swap, send::receipt::Receipt},
};
use serde_json::Value;

#[tokio::test]
async fn both_target_directions_survive_reopen_and_claim_the_exact_signed_operation_once() {
    let buy: Value = serde_json::from_str(include_str!(
        "../../tests/data/base_settlement_target_buy.json"
    ))
    .unwrap();
    let rows: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/data/hashflow_direct_2026-11-26/swaps.json"
    ))
    .unwrap();
    let sell = rows.into_iter().find(|v| v["chain_id"] == 8453).unwrap();
    let folder = std::env::temp_dir().join(format!(
        "arb-settlement-target-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("journal.sqlite");
    for (side, opposite, row) in [(Side::Buy, Side::Sell, buy), (Side::Sell, Side::Buy, sell)] {
        let venue = VenueId("base_dex".into());
        let chain = ChainId("base".into());
        let token = Asset {
            chain: Some(chain.clone()),
            contract: Some(
                "0x4200000000000000000000000000000000000006"
                    .parse()
                    .unwrap(),
            ),
            ticker: "WETH".into(),
        };
        let cash = Asset {
            chain: Some(chain.clone()),
            contract: Some(
                "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
                    .parse()
                    .unwrap(),
            ),
            ticker: "USDC".into(),
        };
        let market = Market {
            evm_chain_id: 8453,
            cex_asset_symbol: None,
            chain,
            cex_network: "BASEEVM".into(),
            cex: VenueId("gate".into()),
            asset: cash,
            quote: "USDT".into(),
            dex: venue.clone(),
        };
        let owner = row["from"].as_str().unwrap();
        let wallet = DexAccountId(owner.into());
        let tx = row["transaction_hash"].as_str().unwrap();
        let receipt: Receipt = serde_json::from_value(row["receipt"].clone()).unwrap();
        let qty = settlement_swap::amount(
            &receipt,
            owner.parse().unwrap(),
            token.contract.unwrap(),
            opposite,
            18,
        )
        .unwrap();
        let quote = settlement_swap::amount(
            &receipt,
            owner.parse().unwrap(),
            market.asset.contract.unwrap(),
            side,
            6,
        )
        .unwrap();
        let nonce = row["nonce"]
            .as_u64()
            .or_else(|| {
                u64::from_str_radix(row["nonce"].as_str()?.trim_start_matches("0x"), 16).ok()
            })
            .unwrap();
        let action = log::Request::Order(Box::new(TradeRequest {
            venue: venue.clone(),
            asset: token,
            side,
            qty,
            worst_price: quote / qty,
            deal_id: tx.into(),
            route: None,
            exit_position: side == Side::Sell,
            remeasure: None,
        }));
        let id = key(&market, tx, &wallet, &venue, side).unwrap();
        let marker = PendingRef {
            nonce: Some(nonce),
            venue_ref: Some(tx.into()),
            ..Default::default()
        };
        {
            let db = Db::open(&file).unwrap();
            let make = || log::scope(&db, &id, action.clone(), log::before_send(&marker));
            let (a, b) = tokio::join!(make(), make());
            assert_ne!(a.is_ok(), b.is_ok());
        }
        {
            let db = Db::open(&file).unwrap();
            let entry = log::load(&db, &id).await.unwrap().unwrap();
            assert_eq!(entry.lookup, marker);
            assert!(entry.result.is_none());
            assert!(
                log::scope(&db, &id, action.clone(), log::before_send(&marker))
                    .await
                    .is_err()
            );
            assert!(
                log::load(&db, &key(&market, tx, &wallet, &venue, opposite).unwrap())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }
    std::fs::remove_dir_all(folder).unwrap();
}
