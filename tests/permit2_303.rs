//! Файл: tests/permit2_303.rs
//! Что проверяет: решение 303 (задача t134) на фактах журнала живого бота 15.11.2026 и официальном исходнике Permit2:
//! 1) селекторы ошибок Permit2 = keccak256 сигнатуры из IAllowanceTransfer.sol (exec/permit2_err.rs — ссылка);
//!    AllowanceExpired(uint256) = 0xd81b2f2e — ровно данные отката из журнала FOREST (сделка 20261115213829104FOREST);
//! 2) расшифровка текста узла из журнала FOREST («код 3: execution reverted: 0xd81b2f2e…6af5f951», returnData пуст —
//!    так записан CallResult в журнале) — срок 12.11.2026 17:37:21 UTC; откат SMARS 0x849eaf98 (журнал 10.11) — не
//!    Permit2; InsufficientAllowance — по данным отката (abi по исходнику);
//! 3) wallet/approve.rs lacking: разрешение Permit2 с тем же сроком 0x6af5f951 к моменту сделки FOREST
//!    (15.11.2026 21:39:14 UTC) — недостающее: ровно один вызов Permit2.approve (ERC-20 → Permit2 уже без ограничения),
//!    новый срок = время блока + dex.permit2_expiration_s; срок в будущем — разрешений не нужно.
//!
//! Не покрыто (записанных данных нет — перечень в docs/TASKS.md 303): ожидание головы опроса блоков до блока approve
//! (exec/approve_seen.rs — нужна запись квитанции approve и последовательности eth_blockNumber того же интервала),
//! eth_simulateV1 пробы после approve на блоке approve и на блоке до него, ответы каналов club48/blockrazor/jetbuilder/
//! nodereal на обмен сразу после approve (случай SIREN).

use alloy::sol_types::SolError;
use alloy_primitives::{Address, U256, address, hex, keccak256};
use serde_json::json;
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::permit2_err::{AllowanceExpired, InsufficientAllowance, explain};
use arb::venues::bsc_dex::wallet::approve::{Read, lacking};

/// Текст отката пробы из журнала FOREST 15.11.2026 21:39:14.123 (core.op_error), часть узла.
const FOREST: &str = "execution reverted: 0xd81b2f2e000000000000000000000000000000000000000000000000000000006af5f951";
/// Срок разрешения из того же отката (0x6af5f951) и время сделки FOREST (журнал, 15.11.2026 21:39:14 UTC).
const EXPIRED: u64 = 0x6af5f951;
const DEAL_TIME: u64 = 1_794_778_754;

const TOKEN: Address = address!("0x00000000000000000000000000000000000000c1");
const PERMIT2: Address = address!("0x00000000000000000000000000000000000000c2");
const UR: Address = address!("0x00000000000000000000000000000000000000c3");

fn sel(sig: &str) -> [u8; 4] {
    keccak256(sig.as_bytes())[..4].try_into().unwrap()
}

async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-permit2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("dex.approve_unlimited", json!(true)),
        ("dex.permit2_expiration_s", json!(3600)),
    ] {
        settings.set(n, v.clone(), Some(val), "тест").await.unwrap();
    }
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

#[test]
fn permit2_selectors_match_source_and_journal() {
    assert_eq!(AllowanceExpired::SELECTOR, sel("AllowanceExpired(uint256)"));
    assert_eq!(
        InsufficientAllowance::SELECTOR,
        sel("InsufficientAllowance(uint256)")
    );
    assert_eq!(hex::encode(AllowanceExpired::SELECTOR), "d81b2f2e");
}

#[test]
fn forest_journal_text_explained() {
    let line = explain(&[], FOREST).expect("AllowanceExpired распознан по тексту узла");
    assert!(line.contains("AllowanceExpired"), "{line}");
    assert!(line.contains("12.11.2026 17:37:21 UTC"), "{line}");
    // Срок — аргумент отката из записанной строки журнала (последнее слово 32 байта); сделка FOREST — позже срока:
    // разрешение действительно истекло к моменту пробы.
    let word = &FOREST[FOREST.len() - 64..];
    let deadline = U256::from_be_slice(&hex::decode(word).unwrap());
    assert_eq!(deadline, U256::from(EXPIRED));
    assert!(U256::from(DEAL_TIME) > deadline);
    // Откат SMARS (журнал 10.11, V2TooLittleReceived) — не Permit2.
    assert!(explain(&[], "execution reverted: 0x849eaf98").is_none());
}

#[test]
fn insufficient_allowance_from_data() {
    let data = InsufficientAllowance {
        amount: U256::from(5u8),
    }
    .abi_encode();
    let line = explain(&data, "execution reverted").expect("InsufficientAllowance");
    assert!(line.contains("разрешено 5 ед."), "{line}");
}

#[tokio::test]
async fn expired_permit2_needs_one_approve() {
    let c = ctx("lacking").await;
    let max160 = (U256::from(1u8) << 160) - U256::from(1u8);
    let read = Read {
        erc20: U256::MAX,
        permit2: max160,
        expiration: EXPIRED,
    };
    let amount = U256::from(1_214_900_000_000_000_000_000u128);
    let out = lacking(&c, &read, (TOKEN, PERMIT2, UR), amount, DEAL_TIME).unwrap();
    assert_eq!(out.len(), 1, "только Permit2.approve");
    let (to, data) = &out[0];
    assert_eq!(*to, PERMIT2);
    assert_eq!(data[..4], sel("approve(address,address,uint160,uint48)"));
    // Слова: token, spender, amount (uint160.max — без ограничения), expiration = время блока + 3600.
    let word = |i: usize| U256::from_be_slice(&data[4 + 32 * i..4 + 32 * (i + 1)]);
    assert_eq!(word(2), max160);
    assert_eq!(word(3), U256::from(DEAL_TIME + 3600));
    // Срок в будущем — разрешений не нужно.
    let fresh = Read {
        expiration: DEAL_TIME + 1,
        ..read
    };
    assert!(
        lacking(&c, &fresh, (TOKEN, PERMIT2, UR), amount, DEAL_TIME)
            .unwrap()
            .is_empty()
    );
}
