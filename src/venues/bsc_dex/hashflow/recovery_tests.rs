//! Реальные квитанции AMM и RFQT. Узел испытания отдаёт только архивные данные, vault закрыт.
use super::super::{
    ctx::{Ctx, Place},
    exec,
    parts::Parts,
    send,
};
use super::{execute, routing, wire};
use crate::{
    core::types::{
        Asset, ChainId, DexRoute, PoolKind, PoolRef, Quote, RouteHop, RouteLeg, RouteMode,
        SendMethod, Side, TradeRequest, TxOutcome,
    },
    gateway::{errors::ErrorCode, pending::PendingRef, settlement_intent as intent},
    secrets::Vault,
    settings::store::Store,
    storage::{db::Db, journal::Journal, raw_log::RawLog},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::sync::broadcast;

fn name(id: u64) -> (&'static str, &'static str) {
    match id {
        1 => ("eth_dex", "ethereum"),
        56 => ("bsc_dex", "bsc"),
        8453 => ("base_dex", "base"),
        42161 => ("arb_dex", "arbitrum"),
        _ => panic!("нет архивной сети"),
    }
}

fn directory(tag: &str, chain: u64) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "arb-hashflow-{tag}-{chain}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
async fn parts(dir: &Path, id: u64, wallet: Address) -> Parts {
    let db = Db::open(&dir.join("state.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(16);
    let (events, _) = broadcast::channel(16);
    let journal = Journal::new(db.clone(), events);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let ctx = Ctx {
        raw: RawLog::start(dir.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        evm_chain: Default::default(),
        place: Place {
            id: name(id).0,
            all: &["bsc_dex", "arb_dex", "base_dex", "eth_dex"],
        },
        chains: Default::default(),
        account: Some(wallet.to_string()),
        index: Default::default(),
        settings,
        vault: Vault::locked(dir.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    ctx.chains.note_gas(if id == 56 { "BNB" } else { "ETH" });
    Parts::new(ctx)
}
#[path="recovery_fixture.rs"]
mod recovery_fixture;
#[derive(Deserialize)]
struct Archive {
    chain_id: u64,
    from: Address,
    input: Bytes,
    nonce: u64,
    transaction_hash: B256,
    receipt: send::receipt::Receipt,
}

/// Смена процесса теряет подпись, но сохраняет исходные nonce, хеш и разбор расхода кошелька.
/// Повтор до reconciliation возвращает прежний идентификатор, после reconciliation — прежний результат, без обращения к vault.
#[tokio::test]
async fn four_networks_recover_a_prepared_transaction_before_accepting_another() {
    let raw: Vec<Archive> = serde_json::from_str(include_str!(
        "../../../../tests/data/hashflow_direct_2026-11-26/swaps.json"
    ))
    .unwrap();
    for r in raw {
        let dir = directory("recovery", r.chain_id);
        let first = parts(&dir, r.chain_id, r.from).await;
        let q = wire::tradeRFQTCall::abi_decode_validate(&r.input)
            .unwrap()
            .quote;
        let qty = q.effectiveBaseTokenAmount;
        let proceeds = qty * q.quoteTokenAmount / q.baseTokenAmount;
        // Эти decimals относятся к зафиксированным парам: USDT/USDC, Binance-Peg ETH/USDC, WETH/USDC, USDT/WETH.
        let (di, dout) = match r.chain_id {
            1 => (6, 6),
            56 => (18, 18),
            8453 => (18, 6),
            42161 => (6, 18),
            _ => unreachable!(),
        };
        let req = TradeRequest {
            venue: first.ctx.venue(),
            asset: Asset {
                chain: Some(ChainId(name(r.chain_id).1.into())),
                ticker: "baseToken".into(),
                contract: Some(q.baseToken),
            },
            side: Side::Sell,
            qty: super::super::num::to_decimal(qty, di).unwrap(),
            worst_price: Decimal::ONE,
            deal_id: r.transaction_hash.to_string(),
            route: Some(DexRoute {
                mode: RouteMode::Rfq(q.txid),
                legs: vec![],
                block: r.receipt.block_number.to(),
                gas_units: r.receipt.gas_used.to(),
                executable: true,
                exec_note: None,
            }),
            exit_position: false,
            remeasure: None,
        };
        let chain = exec::chain_tx::chain_tx(
            r.transaction_hash,
            r.nonce,
            vec![],
            SendMethod::Single,
            &send::track::Tracked::bare(TxOutcome::Pending, None),
            None,
        );
        let mut parse = exec::swap_ctx::SwapCtx::swap(
            r.from,
            (q.baseToken, q.quoteToken),
            (di, dout),
            &[],
            None,
            None,
        );
        parse.chain = Some(chain);
        parse.who = Some("Hashflow RFQT".into());
        parse.rfq=Some(serde_json::from_value(serde_json::json!({"pool":q.pool,"trader":r.from,"input":q.baseToken,"output":q.quoteToken,"txid":q.txid,"amount":qty,"proceeds":proceeds})).unwrap());
        let marker = PendingRef {
            nonce: Some(r.nonce),
            venue_ref: Some(r.transaction_hash.to_string()),
            parse: Some(serde_json::to_value(&parse).unwrap()),
            ..Default::default()
        };
        let key = execute::key(&first, &req).unwrap();
        intent::scope(
            first.ctx.journal.db(),
            &key,
            intent::Request::Order(Box::new(req.clone())),
            intent::before_send(&marker),
        )
        .await
        .unwrap();
        drop(first);
        let restored = parts(&dir, r.chain_id, r.from).await;
        let pending = execute::trade(&restored, req.clone(), q.txid)
            .await
            .unwrap_err();
        assert_eq!(pending.code, ErrorCode::OutcomeUnknown);
        assert_eq!(pending.pending.as_deref(), Some(&marker));
        let entry = intent::load(restored.ctx.journal.db(), &key)
            .await
            .unwrap()
            .unwrap();
        let parse = exec::swap_ctx::SwapCtx::of(&entry.lookup).unwrap();
        let result = exec::swap_rebuild::rebuild(
            &restored.ctx,
            req.clone(),
            &parse,
            r.transaction_hash,
            r.receipt.clone(),
        )
        .unwrap();
        let (canonical,server)=recovery_fixture::start(&restored,&r).await;
        let replay=send::receipt::read(&restored.ctx,r.transaction_hash).await.unwrap().unwrap();
        let proof=parse.rfq.as_ref().unwrap();
        assert!(proof.matches(&replay),"{:?} payment",r.chain_id);
        proof.included(&restored.ctx,r.transaction_hash,r.nonce,&replay).await.unwrap();
        canonical.store(false,std::sync::atomic::Ordering::SeqCst);
        assert_eq!(execute::record(&restored,result.clone()).await.unwrap_err().code,ErrorCode::OutcomeUnknown);
        assert!(intent::load(restored.ctx.journal.db(),&key).await.unwrap().unwrap().result.is_none());
        canonical.store(true,std::sync::atomic::Ordering::SeqCst);
        let stored=execute::record(&restored,result).await.unwrap();
        let settled=intent::load(restored.ctx.journal.db(),&key).await.unwrap().unwrap();
        let Some(intent::Outcome::Order(fill))=settled.result else{panic!("missing confirmed fill");};
        assert_eq!(fill.trade.venue_ref,r.transaction_hash.to_string());
        assert_eq!((fill.trade.filled_qty,fill.trade.avg_price),(stored.filled_qty,stored.avg_price));
        // Учётный профиль заблокирован. Результат читается из журнала, повторное исполнение
        // с неподтверждённым кошельком возвращает исходный идентификатор.
        let retry=execute::trade(&restored,req,q.txid).await.unwrap_err();
        assert_eq!(retry.code,ErrorCode::OutcomeUnknown);
        assert_eq!(retry.pending.as_deref(),Some(&marker));
        server.abort();
    }
}

#[derive(Deserialize)]
struct Ordinary {
    chain: u64,
    tx: B256,
    pool: Address,
    trader: Address,
    coin: Address,
    quote: Address,
    coin_dec: u8,
    quote_dec: u8,
    nonce: u64,
    side: Side,
    amount_in: String,
    amount_out: String,
    receipt: send::receipt::Receipt,
    timestamp: i64,
}

/// Экономический результат обычной сделки V3 в каждой сети при выключенном Hashflow: те же маршрут, цена, объём и расходы газа.
#[tokio::test]
async fn disabled_hashflow_preserves_ordinary_v3_quotes_and_mined_results_on_four_networks() {
    let rows: Vec<Ordinary> = serde_json::from_str(include_str!(
        "../../../../tests/data/hashflow_direct_2026-11-26/amm_controls.json"
    ))
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.chain).collect::<Vec<_>>(),
        [1, 56, 8453, 42161]
    );
    for row in rows {
        let p = parts(&directory("ordinary", row.chain), row.chain, row.trader).await;
        let ctx = &p.ctx;
        assert!(!ctx.flag("hashflow.trading").unwrap_or(false));
        let asset = Asset {
            contract: Some(row.coin),
            chain: Some(ChainId(name(row.chain).1.into())),
            ticker: "token0".into(),
        };
        let i = U256::from_str_radix(&row.amount_in, 10).unwrap();
        let o = U256::from_str_radix(&row.amount_out, 10).unwrap();
        let (coins, cash, tokens) = if row.side == Side::Buy {
            (o, i, (row.quote, row.coin))
        } else {
            (i, o, (row.coin, row.quote))
        };
        let qty = super::super::num::to_decimal(coins, row.coin_dec).unwrap();
        let gross = super::super::num::to_decimal(cash, row.quote_dec).unwrap();
        let price = gross / qty;
        let route = DexRoute {
            mode: RouteMode::Direct,
            block: row.receipt.block_number.to(),
            gas_units: row.receipt.gas_used.to(),
            executable: true,
            exec_note: None,
            legs: vec![RouteLeg {
                share: Decimal::ONE,
                hops: vec![RouteHop {
                    fee_mode: None,
                    family: if row.chain == 56 {
                        "pancakeswap-v3"
                    } else {
                        "uniswap-v3"
                    }
                    .into(),
                    kind: PoolKind::V3,
                    pool: PoolRef::Contract(row.pool),
                }],
                expected_out: if row.side == Side::Buy { qty } else { gross },
            }],
        };
        let baseline = Quote {
            venue: ctx.venue(),
            asset: asset.clone(),
            side: row.side,
            qty,
            fillable_qty: qty,
            avg_price: price,
            costs: vec![],
            route: Some(route.clone()),
            signal_route: None,
            pick: None,
            at: chrono::DateTime::from_timestamp(row.timestamp, 0).unwrap(),
        };
        let expected = serde_json::to_value(&baseline).unwrap();
        let ordinary = crate::gateway::quote_stage::final_scope(routing::consider(
            &p,
            &asset,
            row.side,
            qty,
            Ok(baseline),
        ))
        .await
        .unwrap();
        assert_eq!(serde_json::to_value(&ordinary).unwrap(), expected);
        let req = TradeRequest {
            venue: ctx.venue(),
            asset,
            side: row.side,
            qty,
            worst_price: price,
            deal_id: row.tx.to_string(),
            route: Some(route),
            exit_position: false,
            remeasure: None,
        };
        let tracked = send::track::Tracked {
            outcome: TxOutcome::Included,
            receipt: Some(row.receipt.clone()),
            cancel: None,
            unresolved: None,
        };
        let chain = exec::chain_tx::chain_tx(
            row.tx,
            row.nonce,
            vec![],
            SendMethod::Single,
            &tracked,
            None,
        );
        let done = exec::txs::Done {
            chain,
            receipt: Some(row.receipt),
            cancel: None,
            tip_bnb: None,
            cancels: vec![],
        };
        let r = exec::result::result(
            ctx,
            req,
            done,
            row.trader,
            tokens,
            (row.coin_dec, row.quote_dec),
            &[],
        )
        .unwrap();
        let unchanged = serde_json::to_value(&r).unwrap();
        let stored = execute::record(&p, r).await.unwrap();
        assert_eq!(serde_json::to_value(&stored).unwrap(), unchanged);
        assert_eq!(stored.filled_qty, qty);
        assert_eq!(stored.avg_price, price);
        assert_eq!(stored.venue_ref, row.tx.to_string());
    }
}
