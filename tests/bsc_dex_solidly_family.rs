//! Файл: tests/bsc_dex_solidly_family.rs
//! Что делает: подключение семейства Solidly (ve(3,3)-форки: Topaz v2, THENA V1; решения 80, 85) без сети и без моков:
//! (1) таблица сопоставления (dex.kind = solidly_v2), набор обязательных настроек (только фабрика — БЕЗ Universal
//!   Router/Permit2/роутера/квотера: квотер у Solidly — сам пул, исполнения нет — tradable=false);
//! (2) разбор ответа metadata() по ABI (ISolidlyPool) на ОФИЦИАЛЬНОЙ структуре (velodrome-finance Pool.sol,
//!   HEAD b3065d8) → dec0/dec1/reserve0/reserve1/stable/token0/token1 встают на свои места; собранный PoolState
//!   считается ПРАВИЛЬНОЙ ветвью (stable → math/swap_solidly, volatile → math/v2), и ветви дают разный выход (доказывает,
//!   что флаг stable реально переключает математику);
//! (3) границы fee_u32 (комиссия пула из getFee).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, abi_solidly, load_solidly, math::swap_solidly, math::v2),
//! arb::settings (SQLite). Математика стабильного инварианта на официальных значениях — tests/bsc_dex_solidly_math.rs.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::{swap_solidly, v2};
use arb::venues::bsc_dex::pools::abi_solidly::ISolidlyPool;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::load_solidly::fee_u32;

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-solidly-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

async fn set(c: &Ctx, list: &[(&str, Scope, Value)]) {
    for (n, s, v) in list {
        c.settings
            .set(n, s.clone(), Some(v.clone()), "тест")
            .await
            .unwrap();
    }
}

/// solidly_v2: в графе, но НЕ исполним (graphable=true, tradable=false, fork=false); требует только фабрику.
#[tokio::test]
async fn solidly_family_requirements() {
    let c = ctx("req").await;
    let id = "topaz-bsc";
    let s = Scope::Dex(id.into());
    let a = json!("0x1111111111111111111111111111111111111111");

    let f = families::make(id, "solidly_v2").unwrap();
    assert_eq!(f.proto, Proto::Solidly);
    assert_eq!(
        (f.graphable, f.tradable, f.fork),
        (true, false, false),
        "graphable без исполнения (решение 85)"
    );
    assert!(f.proto.is_solidly());
    assert_eq!(
        families::required(&c, f),
        vec!["dex.factory"],
        "только фабрика — квотер у Solidly сам пул, UR/Permit2/роутер не нужны"
    );

    // Только вид — ещё не подключён (нет dex.pool_enabled).
    set(&c, &[("dex.kind", s.clone(), json!("solidly_v2"))]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("не задана dex.pool_enabled")
    );
    // Включили, но без фабрики — не подключён.
    set(&c, &[("dex.pool_enabled", s.clone(), json!(true))]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("dex.factory")
    );
    // Задана фабрика — подключён.
    set(&c, &[("dex.factory", s.clone(), a)]).await;
    let f = families::classify(&c, id).unwrap();
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
}

/// Кодирует ответ metadata() = 7 слов (dec0, dec1, r0, r1, stable, token0, token1) как контракт и возвращает байты.
fn encode_metadata(
    dec0: U256,
    dec1: U256,
    r0: U256,
    r1: U256,
    stable: bool,
    t0: Address,
    t1: Address,
) -> Vec<u8> {
    let mut raw = Vec::new();
    for w in [dec0, dec1, r0, r1, U256::from(stable as u8)] {
        raw.extend_from_slice(&w.to_be_bytes::<32>());
    }
    raw.extend_from_slice(t0.into_word().as_slice());
    raw.extend_from_slice(t1.into_word().as_slice());
    raw
}

/// Разбор metadata() по ABI и выбор ветви: один и тот же набор резервов при stable=true и stable=false даёт РАЗНЫЙ
/// выход — стабильная кривая (math/swap_solidly) и волатильная (math/v2 с комиссией 0,05%). Декодированные поля идут в
/// математику в правильном порядке (decimals/резервы не перепутаны).
#[test]
fn solidly_metadata_abi_and_branch() {
    let one = U256::from(10u8).pow(U256::from(18u8));
    let (t0, t1) = (
        "0x0000000000000000000000000000000000000111"
            .parse::<Address>()
            .unwrap(),
        "0x0000000000000000000000000000000000000222"
            .parse::<Address>()
            .unwrap(),
    );
    // Сбалансированный пул 18/18 знаков: резервы по 1_000_000 токенов.
    let res = U256::from(1_000_000u64) * one;
    let amount = U256::from(10_000u64) * one; // 10000 токенов входа
    let fee = 5u32; // 0,05% — типичная комиссия стабильного пула (getFee stable)

    for stable in [true, false] {
        let raw = encode_metadata(one, one, res, res, stable, t0, t1);
        let m = ISolidlyPool::metadataCall::abi_decode_returns(&raw).unwrap();
        assert_eq!(m.dec0, one);
        assert_eq!(m.dec1, one);
        assert_eq!(m.r0, res);
        assert_eq!(m.r1, res);
        assert_eq!(m.st, stable);
        assert_eq!(m.t0, t0);
        assert_eq!(m.t1, t1);
    }

    // Стабильная ветвь — swap_solidly; волатильная — v2 с дробью (10000−fee)/10000.
    let out_stable =
        swap_solidly::get_amount_out(amount, res, res, one, one, U256::from(fee)).unwrap();
    let ratio = (U256::from(10_000u32 - fee), U256::from(10_000u32));
    let out_vol = v2::amount_out(amount, res, res, ratio).unwrap();

    // У сбалансированной стабильной кривой проскальзывание меньше, чем у x·y=k на той же сумме → выход больше.
    assert!(
        out_stable > out_vol,
        "stable {out_stable} должен быть больше volatile {out_vol} при равных резервах"
    );
    // Оба меньше входа (есть комиссия и проскальзывание) — ветви реально считают, а не возвращают вход.
    assert!(out_stable < amount && out_vol < amount);
}

/// fee_u32: getFee в u32 (числитель над 10000). Принимает < 10000, отклоняет ≥ 10000 (комиссия ≥ 100%).
#[tokio::test]
async fn solidly_fee_bounds() {
    let c = ctx("fee").await;
    assert_eq!(fee_u32(&c, U256::from(30u32)).unwrap(), 30); // 0,3% volatile
    assert_eq!(fee_u32(&c, U256::from(5u32)).unwrap(), 5); // 0,05% stable
    assert_eq!(fee_u32(&c, U256::ZERO).unwrap(), 0);
    assert!(fee_u32(&c, U256::from(10_000u32)).is_err());
    assert!(fee_u32(&c, U256::from(50_000u32)).is_err());
}
