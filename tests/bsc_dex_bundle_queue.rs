//! Файл: tests/bsc_dex_bundle_queue.rs
//! Что делает: очередь кошелька после итога приватного бандла (send/settle.rs::settle_bundle, решение 84, ревью #82),
//! без реальной сети. Блокер ревью #82: истёкший по maxBlockNumber бандл (all-or-nothing) nonce НЕ расходует, поэтому
//! eth_getTransactionCount не растёт — обычный settle ждал бы его и по таймауту халтил ВСЮ торговлю кошелька, включая
//! классику. Проверяет: итог NotIncluded бандла → очередь свободна сразу (не Halted), место снова берётся (тот же
//! первый nonce), без обращения к узлу; действительно неустановленный исход (Pending/unresolved) → очередь остановлена.
//! Исход конструируется (Tracked::bare) — сеть не нужна (NotIncluded/halt-ветки settle_bundle к узлу не ходят).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::send::{settle, track_util, stats, tx}, wallet::nonce, node::heads,
//! queue_store; arb::secrets (ключ кошелька для Slot), настоящая SQLite во временной папке.

use std::time::Duration;

use alloy_primitives::{B256, U256};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use arb::core::types::{TxOutcome, WalletQueueState};
use arb::secrets::crypto::KdfParams;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::{Ctx, SECRET_WALLET_KEY};
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
const FIRST_NONCE: u64 = 7;
/// Бандл из 2 транзакций: последний nonce = первый + 1.
const LAST_NONCE: u64 = FIRST_NONCE + 1;

/// Окружение с разблокированным хранилищем и ключом кошелька (чтобы очередь выдала место Slot). Узел бутафорский —
/// NotIncluded/halt-ветки settle_bundle к нему не ходят; если бы код ждал роста nonce, тест бы это поймал по Halted.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-bundle-queue-{}-{tag}", std::process::id()));
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
    c
}

/// Бутафорская SendCfg (ветки NotIncluded/halt её полей в цикле не используют).
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

#[tokio::test]
async fn expired_bundle_frees_queue_without_halt() {
    let c = ctx("free").await;
    let nonces = Nonces::default();
    let slot = nonces.acquire(&c).await.expect("место в очереди");
    let hold = slot.hold();
    let (heads, stats, cfg) = (Heads::default(), Stats::default(), cfg());
    let after = After {
        ctx: &c,
        heads: &heads,
        stats: &stats,
        cfg: &cfg,
        from: slot.from,
        nonce: LAST_NONCE,
        hash: B256::repeat_byte(1),
    };
    settle::busy(&after, &hold).await;
    assert!(matches!(nonces.state(), WalletQueueState::Busy { .. }));

    // Истёкший бандл: исход УСТАНОВЛЕН (не в блоке), nonce НЕ заняты. Должно вернуться сразу, без ожидания узла.
    let not_included = Tracked::bare(TxOutcome::NotIncluded, None);
    tokio::time::timeout(
        Duration::from_secs(5),
        settle::settle_bundle(&after, &hold, &not_included),
    )
    .await
    .expect("settle_bundle не ждёт узла при NotIncluded");

    assert_eq!(
        nonces.state(),
        WalletQueueState::Free,
        "очередь свободна — халта нет (ревью #82)"
    );
    assert_eq!(
        queue_store::load(&c).await.unwrap(),
        Some(WalletQueueState::Free),
        "свободное состояние сохранено в SQLite"
    );
    // Место снова берётся (nonce не израсходован — следующая транзакция возьмёт тот же первый nonce у узла).
    drop(hold);
    drop(slot);
    let again = nonces.acquire(&c).await;
    assert!(again.is_ok(), "очередь снова доступна для следующей сделки");
}

#[tokio::test]
async fn unresolved_bundle_halts_queue() {
    // Действительно неустановленный исход (узел молчал за send.queue_max_wait_s) — очередь останавливается, как обычно.
    let c = ctx("halt").await;
    let nonces = Nonces::default();
    let slot = nonces.acquire(&c).await.expect("место в очереди");
    let hold = slot.hold();
    let (heads, stats, cfg) = (Heads::default(), Stats::default(), cfg());
    let after = After {
        ctx: &c,
        heads: &heads,
        stats: &stats,
        cfg: &cfg,
        from: slot.from,
        nonce: LAST_NONCE,
        hash: B256::repeat_byte(2),
    };
    settle::busy(&after, &hold).await;
    let pending = Tracked::bare(TxOutcome::Pending, Some("узел молчал".into()));
    settle::settle_bundle(&after, &hold, &pending).await;
    match nonces.state() {
        WalletQueueState::Halted { nonce, .. } => assert_eq!(nonce, Some(LAST_NONCE)),
        s => panic!("ожидалась остановка очереди, а {s:?}"),
    }
}
