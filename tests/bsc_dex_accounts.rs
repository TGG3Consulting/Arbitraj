//! Файл: tests/bsc_dex_accounts.rs
//! Что делает: адаптер DEX и счета (решение 132): вид на счёт подписывает ключом СВОЕГО кошелька и отдаёт адрес депозита
//! своего кошелька; без привязки к счёту адрес депозита, балансы, очередь и подпись — отказ «кошелёк не выбран» (деньги
//! не уйдут на чужой кошелёк); исполнитель — у кошелька; у каждого кошелька своя очередь (остановка одной не держит
//! другую); нет кошелька — понятная ошибка. Ключи — официальные тестовые векторы: пример EIP-155
//! (https://eips.ethereum.org/EIPS/eip-155, «Example») и первый ключ из документации Hardhat Network
//! (https://hardhat.org/hardhat-network/docs/reference#initial-state, адрес 0xf39F…2266).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (BscDex, ctx, wallet), arb::settings::accounts, arb::secrets.

use alloy_primitives::{Address, address};
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use arb::core::types::{Asset, DexAccountId, WalletQueueState};
use arb::gateway::contract::Venue;
use arb::gateway::errors::ErrorCode;
use arb::secrets::crypto::KdfParams;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::wallet::signer;

const KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};
/// Пример EIP-155: ключ 0x4646…46 → адрес 0x9d8A…4F (из подписи примера).
const KEY_A: &str = "0x4646464646464646464646464646464646464646464646464646464646464646";
/// Hardhat Network, счёт #0.
const KEY_B: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ADDR_B: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
/// Записанный адрес развёрнутого исполнителя из решения 132.
const EXECUTOR: &str = "0x9ef558a5d437718dde44e00990e0efb9976cf794";

/// Окружение с разблокированными секретами (два ключа) и двумя кошельками: «BingX» (прежний секрет, исполнитель) и
/// «MEXC» (новый секрет). Возвращает: окружение без счёта, id кошельков.
async fn setup(tag: &str) -> (Ctx, String, String) {
    let d = std::env::temp_dir().join(format!("arb-dex-acc-{tag}-{}", std::process::id()));
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
    let pw = || Zeroizing::new("pw".to_string());
    c.vault.create(pw(), KDF).await.unwrap();
    let a = c.settings.accounts();
    let bingx = a.reserve_id().await.unwrap();
    let mexc = a.reserve_id().await.unwrap();
    let sb = "bsc_dex.wallet_key";
    let sm = arb::settings::accounts::edit_map::secret_name(&mexc);
    for (s, k) in [(sb, KEY_A), (sm.as_str(), KEY_B)] {
        let v = Some(Zeroizing::new(k.to_string()));
        c.vault.change(pw(), s, v, KDF).await.unwrap();
    }
    a.add(&bingx, "BingX", sb, "bsc_dex", "тест").await.unwrap();
    a.add(&mexc, "MEXC", &sm, &format!("wallet:{mexc}"), "тест")
        .await
        .unwrap();
    a.set_executor(&bingx, Some(EXECUTOR.into()), "тест")
        .await
        .unwrap();
    (c, bingx, mexc)
}

#[tokio::test]
async fn bound_view_uses_own_key_and_unbound_refuses() {
    let (c, bingx, mexc) = setup("bound").await;
    let dex = arb::venues::bsc_dex::BscDex::new(c.clone());
    let cake = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82")),
    };
    // Без счёта: адрес депозита, балансы, очередь — отказ, а не адрес кошелька проб.
    let e = dex.deposit_address(&cake, "BEP20").await.unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert!(e.message.contains("не выбран"), "{}", e.message);
    assert!(dex.balances().await.is_err());
    assert!(dex.wallet_queue().await.is_err());
    // Вид на счёт MEXC — адрес ключа MEXC; на счёт BingX — адрес ключа BingX.
    let vm = dex.for_account(&DexAccountId(mexc.clone())).unwrap();
    let got = vm.deposit_address(&cake, "BEP20").await.unwrap();
    assert_eq!(got.parse::<Address>().unwrap(), ADDR_B);
    let vb = dex.for_account(&DexAccountId(bingx.clone())).unwrap();
    let a_addr = signer::address(&c.with_account(&bingx)).unwrap();
    assert_ne!(a_addr, ADDR_B);
    let got = vb.deposit_address(&cake, "BEP20").await.unwrap();
    assert_eq!(got.parse::<Address>().unwrap(), a_addr);
    // Нет такого кошелька — понятная ошибка.
    let e = dex.for_account(&DexAccountId("w999".into())).err().unwrap();
    assert!(e.message.contains("w999"), "{}", e.message);
}

#[tokio::test]
async fn executor_and_queue_are_per_wallet() {
    let (c, bingx, mexc) = setup("queue").await;
    let x: Address = EXECUTOR.parse().unwrap();
    assert_eq!(c.with_account(&bingx).executor().unwrap(), Some(x));
    assert_eq!(
        c.with_account(&mexc).executor().unwrap(),
        None,
        "у MEXC свой исполнитель — не развёрнут"
    );
    // Подпись без счёта — отказ.
    let tx = alloy::consensus::TxLegacy::default();
    assert!(signer::sign(&c, tx).is_err());
    // Очереди разные: остановка очереди BingX не держит MEXC.
    let p = arb::venues::bsc_dex::parts::Parts::new(c.clone());
    let (pb, pm) = (p.bound(&bingx), p.bound(&mexc));
    let slot = pb.nonces.acquire(&pb.ctx).await.unwrap();
    slot.hold().halt("тест".into(), 1, Default::default());
    drop(slot);
    assert!(matches!(pb.nonces.state(), WalletQueueState::Halted { .. }));
    assert_eq!(pm.nonces.state(), WalletQueueState::Free);
    let s = pm
        .nonces
        .acquire(&pm.ctx)
        .await
        .expect("очередь MEXC свободна");
    assert_eq!(s.from, ADDR_B);
    // Повторная привязка — то же состояние кошелька (одна очередь на кошелёк).
    assert!(matches!(
        p.bound(&bingx).nonces.state(),
        WalletQueueState::Halted { .. }
    ));
}
