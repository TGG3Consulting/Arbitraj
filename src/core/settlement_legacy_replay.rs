use super::*;
use crate::core::deal::Deal;
use crate::core::deps::Deps;
use crate::core::types::{CoinNetworkInfo, Cost, CostKind, DexAccountId, TradeResult, Transfer};
use crate::core::{persist, withdraw_seen};
use crate::gateway::registry::Registry;
use crate::notify::dispatch::Notifier;
use crate::settings::{store::Store, value::Scope};
use crate::storage::{db::Db, journal::Journal};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use tokio::sync::broadcast;

#[derive(Deserialize)]
struct Entry {
    direction: Direction,
    dex_account: DexAccountId,
    buy: TradeResult,
}

#[derive(Deserialize)]
struct Pair {
    sent: Transfer,
    credited: Transfer,
    withdraw_requested: Decimal,
}

async fn open(path: &Path) -> Core {
    let db = Db::open(&path.join("ledger.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(32);
    let (tx, _) = broadcast::channel(32);
    let journal = Journal::new(db.clone(), tx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: crate::secrets::Vault::locked(path.join("secrets.enc")),
        journal: journal.clone(),
    };
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 32,
        data_dir: path.into(),
    })
    .await
}

fn entry(json: &str, pair: &Pair) -> Deal {
    let e: Entry = serde_json::from_str(json).unwrap();
    let mut d = Deal::new(
        e.buy.request.deal_id.clone(),
        e.direction,
        Value::Null,
        Decimal::ZERO,
    );
    d.dex_account = Some(e.dex_account);
    d.buy = Some(e.buy);
    d.transfers = vec![pair.sent.clone(), pair.credited.clone()];
    d
}

fn receipt(rows: &[Value], d: &Deal) -> (Decimal, Decimal) {
    let credit = &d.transfers[1];
    let hash = credit.tx_hash.as_ref().unwrap();
    let rec = rows
        .iter()
        .find(|r| r["transactionHash"].as_str() == Some(hash.as_str()))
        .unwrap();
    let event = rec["logs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|log| {
            log["topics"][0] == "0x0226a2f5c1ae0e071aeec3d4ebafcefdc5c549be11f40ed27e76e802acccf374"
        })
        .unwrap();
    let (raw, ui) = event["data"]
        .as_str()
        .unwrap()
        .strip_prefix("0x")
        .unwrap()
        .split_at(64);
    let value = |s| Decimal::from_i128_with_scale(i128::from_str_radix(s, 16).unwrap(), 18);
    assert_eq!(value(raw), credit.amount);
    (value(ui), value(ui) / value(raw))
}

// Публикация той же структуры, что после refresh. Квитанции архивные,
// шлюз пустой: даже в проверке истории сетевого обращения не требуется.
async fn restore(core: &Core, deals: &[Deal], proofs: &[(Decimal, Decimal)]) {
    let asset = &deals[0].direction.asset;
    let mut observations = vec![];
    let receipts = Arc::new(history::Receipts::default());
    for (deal, proof) in deals.iter().zip(proofs) {
        let dex = &deal.direction.sell_on;
        let recv = &deal.transfers[1];
        observations.push(history::observed(deal, dex).unwrap());
        lk(&receipts).insert(
            (
                dex.clone(),
                deal.dex_account.clone().unwrap(),
                true,
                recv.tx_hash.clone().unwrap(),
                recv.amount,
            ),
            *proof,
        );
    }
    let (modes, legacy, pending) =
        history::verified(core, asset, Decimal::from(8), &observations, &receipts).await;
    assert!(!pending);
    let snapshot = Snapshot {
        height: 135478487,
        decimals: 18,
        multiplier: Some(Decimal::from(8)),
        modes: Arc::new(modes),
        legacy: Arc::new(legacy),
        declared: Default::default(),
        sources: Default::default(),
        revision: observations.len(),
        expires: None,
        pending_evidence: pending,
    };
    lk(&core.inner.settlement.rows).insert(
        asset.clone(),
        Record {
            snapshot: Some(snapshot),
            observations,
            receipts,
            ..Record::default()
        },
    );
}

fn arrived(core: &Core, d: &Direction, from: &withdraw_seen::Seen) -> Decimal {
    let info: CoinNetworkInfo = serde_json::from_value(json!({
        "venue":d.buy_on,"asset":d.asset,"network":"BSC","withdraw_enabled":true,"deposit_enabled":true,
        "withdraw_fee":from.fee,"updated_at":"2026-11-24T14:25:39Z"
    })).unwrap();
    let fee = Cost::new(
        CostKind::WithdrawFee,
        from.fee.unwrap(),
        d.asset.ticker.clone(),
        false,
    );
    let costs = withdraw_seen::adjust_route(
        core,
        (&d.buy_on, Some(&d.sell_on)),
        &d.asset,
        &info,
        Decimal::ZERO,
        from.sent,
        vec![fee],
    );
    let spent: Decimal = costs.iter().map(|c| c.amount).sum();
    (from.sent - spent) / Decimal::from(8)
}

#[tokio::test]
async fn both_recorded_withdrawals_restore_without_replacing_history_after_restart() {
    let pairs: Vec<Pair> = serde_json::from_str(include_str!(
        "../../tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json"
    ))
    .unwrap();
    let chain: Vec<Value> = serde_json::from_str(include_str!(
        "../../tests/data/bsc_units_2026_11_24/deposit_receipts.json"
    ))
    .unwrap();
    let mut deals = [
        entry(
            include_str!("../../tests/data/bsc_units_2026_11_24/deal_entry.json"),
            &pairs[0],
        ),
        entry(
            include_str!("../../tests/data/bsc_units_2026_11_24/deal_add_entry.json"),
            &pairs[1],
        ),
    ];
    let mut proofs = deals.each_ref().map(|d| receipt(&chain, d));
    let dir = deals[0].direction.clone();
    let folder = std::env::temp_dir().join(format!(
        "arb-unit-legacy-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros()
    ));
    std::fs::create_dir_all(&folder).unwrap();
    for p in &pairs {
        let raw = withdraw_seen::Seen {
            asset: dir.asset.clone(),
            from: dir.buy_on.clone(),
            sent: p.withdraw_requested,
            credited: p.credited.amount,
            fee: Some(p.sent.fee.as_ref().unwrap().amount),
        };
        let saved = open(&folder).await;
        withdraw_seen::record(&saved, raw.clone()).await;
        saved
            .inner
            .deps
            .settings
            .set(
                "transfer.seen_cost",
                Scope::Venue(dir.buy_on.0.clone()),
                Some(json!(true)),
                "test",
            )
            .await
            .unwrap();
        drop(saved);
        for _ in 0..2 {
            let core = open(&folder).await;
            let to = Some(&dir.sell_on);
            assert_eq!(
                withdraw_seen::get_for(&core, &dir.asset, &dir.buy_on, to),
                Some(raw.clone())
            );
            restore(&core, &deals, &proofs).await;
            let fact = withdraw_seen::get_for(&core, &dir.asset, &dir.buy_on, to).unwrap();
            assert_eq!(fact.sent, p.sent.amount);
            assert_eq!(fact.credited, p.sent.amount - raw.fee.unwrap());
            assert_eq!(arrived(&core, &dir, &fact), p.credited.amount);
            assert!(
                withdraw_seen::get_for(
                    &core,
                    &dir.asset,
                    &dir.buy_on,
                    Some(&VenueId("other".into()))
                )
                .is_none()
            );
            assert!(withdraw_seen::get_for(&core, &dir.asset, &dir.buy_on, None).is_none());
            assert_eq!(
                persist::load::<withdraw_seen::Seen>(&core.inner.deps.db, "withdraw_fact")
                    .await
                    .unwrap(),
                vec![raw.clone()]
            );
        }
    }

    // Контрпример на базе второй карточки. Тот же множитель и независимая
    // первая квитанция, но во второй до кошелька дошло только 98% монет.
    let core = open(&folder).await;
    let p = &pairs[1];
    let keep = Decimal::new(98, 2);
    deals[1].transfers[1].amount *= keep;
    proofs[1].0 *= keep;
    let partial = withdraw_seen::Seen {
        asset: dir.asset.clone(),
        from: dir.buy_on.clone(),
        sent: p.withdraw_requested,
        credited: deals[1].transfers[1].amount,
        fee: Some(p.sent.fee.as_ref().unwrap().amount),
    };
    withdraw_seen::record(&core, partial.clone()).await;
    // Одна неполная операция не устанавливает единицу отправителя.
    restore(&core, &deals[1..], &proofs[1..]).await;
    let read =
        || withdraw_seen::get_for(&core, &dir.asset, &dir.buy_on, Some(&dir.sell_on)).unwrap();
    assert_eq!(read(), partial);
    restore(&core, &deals, &proofs).await;
    let measured = read();
    assert_eq!(
        measured.credited,
        (measured.sent - measured.fee.unwrap()) * keep
    );
    assert_eq!(arrived(&core, &dir, &measured), partial.credited);
    // Одни и те же числа у разных операций не определяют хеш старого withdraw_fact.
    let mut duplicate = deals[1].clone();
    duplicate.transfers[0].tx_hash = deals[0].transfers[0].tx_hash.clone();
    duplicate.transfers[1].tx_hash = duplicate.transfers[0].tx_hash.clone();
    restore(
        &core,
        &[deals[0].clone(), deals[1].clone(), duplicate],
        &[proofs[0], proofs[1], proofs[1]],
    )
    .await;
    assert_eq!(read(), partial);
    restore(&core, &deals, &proofs).await;

    // Неидентифицируемый старый факт и известный более свежий факт маршрута сохраняют приоритет.
    let other = withdraw_seen::Seen {
        sent: partial.sent + Decimal::ONE,
        ..partial.clone()
    };
    withdraw_seen::record(&core, other.clone()).await;
    assert_eq!(read(), other);
    withdraw_seen::record(&core, partial.clone()).await;
    withdraw_seen::record_for(
        &core,
        &dir.sell_on,
        other.clone(),
        Some(withdraw_seen::Receipt {
            tx: p.credited.tx_hash.clone().unwrap(),
            raw: partial.credited,
            requested: other.sent,
            venue_units: None,
        }),
    )
    .await;
    assert_eq!(read(), other);
}
