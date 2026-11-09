//! Файл: tests/bsc_dex_tracker_abort.rs
//! Что проверяет: задача трекера транзакции кошелька DEX прервана (паника внутри неё, паника мимо защиты, отмена задачи
//! tokio) ПОСЛЕ отправки — очередь кошелька ОСТАНОВЛЕНА (Halted, причина «отслеживание транзакции <хеш> прервано: …»,
//! nonce и хеш, состояние в SQLite), следующая транзакция поверх не уходит (acquire → QueueHalted); итог трекера —
//! Pending с причиной, из которого путь делает «исход неизвестен» с «чем найти» (одиночная — exec/chain_tx.rs +
//! exec/swap_ctx.rs seed, как exec/txs.rs; бандл — exec/bundle_trade.rs bundle_result + BundleRef, как
//! exec/bundle_send.rs). Паника — настоящая (panic! в задаче tokio), очередь, удержания, SQLite — настоящие (wallet/nonce.rs,
//! send/settle.rs guarded / joined — те же функции, что зовут exec/txs.rs и exec/bundle_send.rs); узел не нужен: ветки
//! остановки к нему не ходят. Хеши «чем найти» — настоящие (tests/data/receipts_2026-10-25/responses.json, блок 129702239,
//! транзакции индексов 5 и 6 отправителя 0x0fe0…).
//! Было (до правки): задача трекера падала — вызывающий получал Internal без факта транзакции, удержание пропадало вместе
//! с задачей, состояние оставалось Busy — acquire пропускал следующую транзакцию.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{send::{settle, track_util, stats, tx}, wallet::{nonce, queue_store}, node::heads,
//! exec::{bundle_ref, bundle_send, bundle_trade, chain_tx, swap_ctx}}, arb::secrets (ключ кошелька для Slot), настоящая SQLite.

use std::str::FromStr;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256, address};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};
use zeroize::Zeroizing;

use arb::core::types::{
    Asset, SendMethod, Side, TradeRequest, TxOutcome, VenueId, WalletQueueState,
};
use arb::gateway::errors::ErrorCode;
use arb::secrets::crypto::KdfParams;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::{Ctx, SECRET_WALLET_KEY};
use arb::venues::bsc_dex::exec::bundle_ref::BundleRef;
use arb::venues::bsc_dex::exec::bundle_send::Sent;
use arb::venues::bsc_dex::exec::swap_ctx::{self, SwapCtx};
use arb::venues::bsc_dex::exec::{bundle_trade, chain_tx};
use arb::venues::bsc_dex::node::heads::Heads;
use arb::venues::bsc_dex::send::settle::{self, After};
use arb::venues::bsc_dex::send::stats::Stats;
use arb::venues::bsc_dex::send::track_util::Tracked;
use arb::venues::bsc_dex::send::tx::SendCfg;
use arb::venues::bsc_dex::wallet::nonce::Nonces;
use arb::venues::bsc_dex::wallet::queue_store;

/// Параметры Argon2id маленькие ради скорости (как tests/secrets_vault.rs).
const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};
/// Тестовый ключ из официального примера EIP-155 (wallet/signer.rs).
const KEY: &str = "0x4646464646464646464646464646464646464646464646464646464646464646";
const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";
const NONCE: u64 = 41;
/// USDT BSC — токен двух записанных транзакций (взгляд теста: базовая монета).
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");

/// Хеши двух записанных транзакций (results[8], индексы 5 и 6). Паника — файла нет.
fn hashes() -> Vec<B256> {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(RECEIPTS).unwrap()).unwrap();
    [5, 6]
        .iter()
        .map(|&i| {
            let h = rec["results"][8]["response"]["result"][i]["transactionHash"].as_str();
            B256::from_str(h.unwrap()).unwrap()
        })
        .collect()
}

/// Окружение с разблокированным хранилищем и ключом кошелька (как tests/bsc_dex_bundle_queue.rs).
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-tracker-abort-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    c.vault
        .create(Zeroizing::new("pw".into()), KDF)
        .await
        .unwrap();
    c.vault
        .change(
            Zeroizing::new("pw".into()),
            SECRET_WALLET_KEY,
            Some(Zeroizing::new(KEY.into())),
            KDF,
        )
        .await
        .unwrap();
    let a = c.settings.accounts();
    let id = a.reserve_id().await.unwrap();
    a.add(&id, "тест", SECRET_WALLET_KEY, "bsc_dex", "тест")
        .await
        .unwrap();
    c.with_account(&id)
}

/// SendCfg (ветки остановки её полей не читают).
fn cfg() -> SendCfg {
    SendCfg {
        max_wait: 10,
        bundle_max: 3,
        bump_pct: Decimal::ZERO,
        stale: Duration::from_millis(50),
        replaced_confirm: 1,
        cancel_max_wei: U256::ZERO,
        cancel_max_attempts: 1,
        queue_max_wait: Duration::from_secs(60),
    }
}

/// Очередь остановлена этой транзакцией, причина — «отслеживание … прервано: <что>», состояние сохранено в SQLite.
async fn assert_halted(c: &Ctx, q: &Nonces, hash: B256, what: &str) -> String {
    let st = q.state();
    let WalletQueueState::Halted {
        reason,
        nonce,
        hash: h,
        ..
    } = st.clone()
    else {
        panic!("ожидалась остановка очереди, а {st:?}");
    };
    assert_eq!(
        (nonce, h),
        (Some(NONCE), Some(hash)),
        "nonce и хеш этой транзакции"
    );
    let head = format!("отслеживание транзакции {hash} прервано: ");
    assert!(reason.starts_with(&head), "причина: {reason}");
    assert!(reason.contains(what), "причина: {reason}");
    assert_eq!(
        queue_store::load(c).await.unwrap(),
        Some(st),
        "остановка сохранена в SQLite (переживает перезапуск)"
    );
    reason
}

/// Следующая транзакция поверх не уходит: место не выдаётся, код QueueHalted.
async fn assert_blocked(c: &Ctx, q: &Nonces) {
    match tokio::time::timeout(Duration::from_secs(5), q.acquire(c)).await {
        Ok(Err(e)) => assert_eq!(e.code, ErrorCode::QueueHalted, "{}", e.message),
        Ok(Ok(_)) => panic!("очередь выдала место поверх транзакции с неизвестным исходом"),
        Err(_) => panic!("очередь так и не освободилась (удержание не отпущено)"),
    }
}

/// Паника внутри слежения (задача держит удержание, как в exec/txs.rs): guarded останавливает очередь, joined у
/// вызывающего отдаёт причину; из неё — итог Pending → «исход неизвестен» с «чем найти» (хеш, nonce), как txs.rs.
#[tokio::test]
async fn panic_inside_tracker_halts_queue() {
    let c = ctx("inside").await;
    let q = Nonces::default();
    let slot = q.acquire(&c).await.expect("место в очереди");
    let hash = hashes()[0];
    let (heads, stats, cfg) = (Heads::default(), Stats::default(), cfg());
    let after = After {
        ctx: &c,
        heads: &heads,
        stats: &stats,
        cfg: &cfg,
        from: slot.from,
        nonce: NONCE,
        hash,
    };
    let hold = slot.hold();
    settle::busy(&after, &hold).await;
    let hold_out = hold.clone();
    let (tc, from) = (c.clone(), slot.from);
    let job = tokio::spawn(async move {
        let (heads, stats) = (Heads::default(), Stats::default());
        let a = After {
            ctx: &tc,
            heads: &heads,
            stats: &stats,
            cfg: &cfg,
            from,
            nonce: NONCE,
            hash,
        };
        settle::guarded(&a, &hold, async {
            if hash != B256::ZERO {
                panic!("разбор квитанции упал");
            }
            Tracked::bare(TxOutcome::Included, None)
        })
        .await
    });
    let why = settle::joined(&after, &hold_out, job)
        .await
        .err()
        .expect("слежение прервано — итога нет");
    let reason = assert_halted(&c, &q, hash, "паника задачи трекера: разбор квитанции упал").await;
    assert_eq!(why, reason, "вызывающий получает ту же причину");

    // Как exec/txs.rs: итог Pending с причиной → факт транзакции и «чем найти».
    let t = Tracked::bare(TxOutcome::Pending, Some(why));
    let chain = chain_tx::chain_tx(
        hash,
        NONCE,
        vec!["48club".into()],
        SendMethod::Single,
        &t,
        None,
    );
    assert_eq!(chain.outcome, TxOutcome::Pending);
    let seed = swap_ctx::seed(&chain, None);
    assert_eq!(seed.venue_ref.as_deref(), Some(hash.to_string().as_str()));
    assert_eq!(seed.nonce, Some(NONCE));
    assert!(
        t.unresolved.is_some(),
        "txs.rs отдаёт OutcomeUnknown именно по unresolved"
    );

    drop(hold_out);
    drop(slot);
    assert_blocked(&c, &q).await;
}

/// Вызывающий уже прерван (сделка отменена — JoinHandle и место отпущены), а задача трекера потом паникует: очередь
/// всё равно остановлена — guarded ловит панику внутри задачи, пока её удержание живо. Раньше: Busy, место свободно.
#[tokio::test]
async fn panic_after_caller_dropped_still_halts() {
    let c = ctx("dropped").await;
    let q = Nonces::default();
    let slot = q.acquire(&c).await.expect("место в очереди");
    let hash = hashes()[1];
    let (heads, stats, cfg) = (Heads::default(), Stats::default(), cfg());
    let hold = slot.hold();
    let after = After {
        ctx: &c,
        heads: &heads,
        stats: &stats,
        cfg: &cfg,
        from: slot.from,
        nonce: NONCE,
        hash,
    };
    settle::busy(&after, &hold).await;
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (tc, from) = (c.clone(), slot.from);
    let job = tokio::spawn(async move {
        let (heads, stats) = (Heads::default(), Stats::default());
        let a = After {
            ctx: &tc,
            heads: &heads,
            stats: &stats,
            cfg: &cfg,
            from,
            nonce: NONCE,
            hash,
        };
        settle::guarded(&a, &hold, async {
            let _ = go_rx.await;
            panic!("{}", "квитанция не разобрана".to_string());
        })
        .await
    });
    drop(job);
    drop(slot);
    go_tx.send(()).unwrap();
    // Задача сама остановит очередь и отпустит удержание — acquire дождётся замка и получит отказ.
    assert_blocked(&c, &q).await;
    assert_halted(
        &c,
        &q,
        hash,
        "паника задачи трекера: квитанция не разобрана",
    )
    .await;
}

/// Задача упала мимо защиты (паника вне guarded) и задача отменена (abort): joined у вызывающего останавливает очередь
/// своим удержанием; из причины — итог бандла Pending → «исход неизвестен» с «чем найти» бандла (как bundle_send.rs).
#[tokio::test]
async fn panic_outside_guard_and_cancel_halt_queue() {
    let c = ctx("outside").await;
    let q = Nonces::default();
    let slot = q.acquire(&c).await.expect("место в очереди");
    let hs = hashes();
    let last = *hs.last().unwrap();
    let (heads, stats, cfg) = (Heads::default(), Stats::default(), cfg());
    let after = After {
        ctx: &c,
        heads: &heads,
        stats: &stats,
        cfg: &cfg,
        from: slot.from,
        nonce: NONCE,
        hash: last,
    };
    let hold = slot.hold();
    settle::busy(&after, &hold).await;
    let held = hold.clone();
    let job = tokio::spawn(async move {
        let _h = held;
        if last != B256::ZERO {
            panic!("задача трекера упала до защиты");
        }
        Ok::<(Tracked, Vec<()>), String>((Tracked::bare(TxOutcome::Included, None), Vec::new()))
    });
    let why = settle::joined(&after, &hold, job)
        .await
        .err()
        .expect("прервано");
    assert_halted(
        &c,
        &q,
        last,
        "паника задачи трекера: задача трекера упала до защиты",
    )
    .await;

    // Отмена задачи (abort) — та же остановка.
    let pending = tokio::spawn(std::future::pending::<Result<(), String>>());
    pending.abort();
    let cancelled = settle::joined(&after, &hold, pending)
        .await
        .expect_err("отменена");
    assert!(cancelled.contains("задача трекера отменена"), "{cancelled}");
    assert_halted(&c, &q, last, "задача трекера отменена").await;

    // Как exec/bundle_send.rs → exec/bundle_trade.rs: Pending с причиной → OutcomeUnknown с «чем найти» бандла.
    let sent = Sent {
        hashes: hs.clone(),
        channels: vec!["48club".into()],
        tracked: Tracked::bare(TxOutcome::Pending, Some(why)),
        receipts: Vec::new(),
        max_block: 129_702_242,
    };
    let first = NONCE - 1;
    let req = TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        side: Side::Sell,
        qty: Decimal::ONE,
        worst_price: Decimal::ONE,
        deal_id: "20261108120000000USDT".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    };
    let mut sc = SwapCtx::swap(after.from, (USDT, USDT), (18, 18), &[], None, None);
    sc.bundle = Some(BundleRef::of(&sent, first));
    let e = bundle_trade::bundle_result(
        &c,
        req,
        &sent,
        after.from,
        (USDT, USDT),
        (18, 18),
        (first, &[]),
    )
    .map_err(|e| sc.attach(e))
    .expect_err("исход неизвестен");
    assert_eq!(e.code, ErrorCode::OutcomeUnknown, "{}", e.message);
    assert!(e.message.contains("прервано"), "{}", e.message);
    let p = e.pending.as_deref().expect("«чем найти»");
    let got = SwapCtx::of(p)
        .and_then(|c| c.bundle)
        .expect("BundleRef в контексте");
    assert_eq!(got.hashes, hs, "хеши всех транзакций бандла");
    assert_eq!((got.first_nonce, got.last_nonce()), (first, NONCE));

    drop(hold);
    drop(slot);
    assert_blocked(&c, &q).await;
}
