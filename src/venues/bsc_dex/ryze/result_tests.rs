//! Конечное движение средств в SpotFill по архивным квитанциям. GET статуса записан для одной из трёх заявок.
use super::{chain::Mined, fill, ledger::Marker, result, wire::executeSwapIntentsCall};
use crate::{
    core::types::{Asset, ChainId, Side, TradeRequest, TradeStatus, TxOutcome},
    venues::bsc_dex::{hashflow::recovery_tests, send::receipt::Receipt},
};
use alloy::{primitives::{Address, B256, Bytes}, sol_types::SolCall};
use rust_decimal::Decimal;
use serde_json::Value;

fn archive(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}

#[tokio::test]
async fn completed_intent_reports_wallet_cash_refund_and_relayer_gas() {
    let receipts = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_historical_receipts.json.gz"
    ));
    let verified = archive(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_base_verified_intents.json.gz"
    ));
    let service: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ryze_service_responses.json"
    )).unwrap();
    let status = &service["relayer_status"]["body"]["intent"];
    let mut linked_status = 0;
    let mut refunds = 0;
    let facts = verified["cases"].as_array().unwrap();
    assert_eq!(facts.len(), 3);
    for fact in facts {
        let hash: B256 = fact["transaction"].as_str().unwrap().parse().unwrap();
        let row = receipts.as_array().unwrap().iter()
            .find(|r| r["transactionHash"] == fact["transaction"]).unwrap();
        let receipt: Receipt = serde_json::from_value(row["receipt"].clone()).unwrap();
        assert_eq!(receipt.transaction_hash, Some(hash));
        assert_eq!(row["receipt"]["blockHash"], fact["block_hash"]);
        let tx = &row["transaction"];
        let input: Bytes = tx["input"].as_str().unwrap().parse().unwrap();
        let batch = executeSwapIntentsCall::abi_decode_validate(&input).unwrap();
        let router: Address = tx["to"].as_str().unwrap().parse().unwrap();
        let digest: B256 = fact["digest"].as_str().unwrap().parse().unwrap();
        let order = batch.signedIntents.into_iter().map(|s| s.intent)
            .find(|o| o.digest(super::CHAIN_ID, router) == digest).unwrap();
        assert!(order.present_in(router, &input));
        assert_eq!(tx["from"], fact["gas_paid_by"]);
        assert_ne!(order.user, tx["from"].as_str().unwrap().parse::<Address>().unwrap());
        if status["intentId"] == fact["digest"] {
            assert_eq!(status["status"], "confirmed");
            assert_eq!(status["txHash"], fact["transaction"]);
            linked_status += 1;
        }
        let fill = fill::confirmed(&order, router, &receipt).unwrap();
        let usdc = service["settings_pools"][0][2].as_str().unwrap().parse::<Address>().unwrap();
        let (side, coin, decimals, ticker) = if order.tokenIn == usdc {
            (Side::Buy, order.tokenOut, 18, "WETH")
        } else {
            assert_eq!(order.tokenOut, usdc);
            let weth = service["settings_pools"][0][1].as_str().unwrap().parse::<Address>().unwrap();
            if order.tokenIn == weth {
                (Side::Sell, weth, 18, "WETH")
            } else {
                let btc = service["settings_pools"][1][1].as_str().unwrap().parse::<Address>().unwrap();
                assert_eq!(order.tokenIn, btc);
                (Side::Sell, btc, 8, "cbBTC")
            }
        };
        let dir = recovery_tests::directory("ryze-archived-spot-fill", super::CHAIN_ID);
        let p = recovery_tests::parts(&dir, super::CHAIN_ID, order.user).await;
        let units = |v: &Value, scale| Decimal::from_i128_with_scale(
            v.as_str().unwrap().parse().unwrap(), scale
        );
        // Учётная заявка для исторического кошелька: BUY задаёт минимум получения, SELL — весь вход.
        let (request_qty, cash_bound) = if side == Side::Buy {
            (units(&fact["minimum"], decimals), units(&fact["input"], 6))
        } else {
            (units(&fact["input"], decimals), units(&fact["minimum"], 6))
        };
        let req = TradeRequest {
            venue: p.ctx.venue(),
            asset: Asset {
                chain: Some(ChainId("base".into())),
                contract: Some(coin),
                ticker: ticker.into(),
            },
            side,
            qty: request_qty,
            worst_price: cash_bound.checked_div(request_qty).unwrap(),
            deal_id: hash.to_string(),
            route: None,
            exit_position: false,
            remeasure: None,
        };
        let expected_debit = units(&fact["wallet_net_spent"], if side == Side::Buy { 6 } else { decimals });
        let expected_credit = units(&fact["net"], if side == Side::Buy { decimals } else { 6 });
        let (qty, cash) = if side == Side::Buy { (expected_credit, expected_debit) } else { (expected_debit, expected_credit) };
        if side == Side::Buy {
            assert!(qty > req.qty);
        } else if qty < req.qty {
            assert_eq!(req.qty - qty, units(&fact["additional_pool_input"], decimals));
            refunds += 1;
        }
        // Из маркера result::filled читает только decimals. Учётный ключ и nonce транзакции релея
        // отсутствуют в экспорте; в этой проверке они не используются для определения суммы или исхода.
        let m = Marker {
            key: hash.to_string(), account: p.ctx.account.clone().unwrap(), router,
            runtime: B256::ZERO, relayer: String::new(), digest, tokens: vec![], order,
            start: 0, start_hash: B256::ZERO, decimals: (u8::try_from(decimals).unwrap(), 6),
        };
        let gas = receipt.gas_used.to::<u64>();
        let done = result::filled(&p, req, &m, Mined { hash, tx_nonce: 0, fill, receipt }).unwrap();
        assert_eq!(done.trade.status, TradeStatus::Filled);
        assert_eq!(done.trade.filled_qty, qty);
        assert_eq!(done.gross_quote, cash);
        assert_eq!(done.trade.avg_price, cash.checked_div(qty).unwrap());
        assert!(done.trade.fees.is_empty());
        let tx = done.trade.chain.unwrap();
        assert_eq!(tx.hash, hash);
        assert_eq!(tx.outcome, TxOutcome::Included);
        assert!(gas > 0);
        assert_eq!(tx.gas_used, Some(gas));
        assert_eq!(tx.gas_bnb, Some(Decimal::ZERO));
    }
    assert_eq!((linked_status, refunds), (1, 1));
}
