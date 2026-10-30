//! Файл: tests/bsc_dex_algebra_family.rs
//! Что делает: подключение семейства Algebra (THENA Fusion, линия Algebra V1.9; решения 80, 81) без сети и без моков:
//! (1) таблица сопоставления (dex.kind = thena_fusion), набор обязательных настроек (фабрика + квотер, БЕЗ Universal
//!   Router/Permit2/роутера/читателя тиков — graphable, но НЕ tradable, исполнения пока нет);
//! (2) разбор ответа globalState()/ticks() по ABI (IAlgebraPool) на структуре официального контракта
//!   (@cryptoalgebra/v1.9-core) — поля встают на свои места;
//! (3) восстановление СЫРЫХ тиков из слова битмапа tickTable (raw_ticks): бит b слова w → тик w·256 + b, без tickSpacing;
//! (4) проверка пула через фабрику Algebra (verify_algebra::decide на ABI-ответе poolByPair) — принимает свой пул,
//!   отвергает чужой.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, abi_algebra, load_algebra, verify_algebra, verify, discover),
//! arb::settings (настоящая SQLite во временной папке).
//! Математика Algebra на официальных значениях — tests/bsc_dex_algebra_math.rs.

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, I256, U256};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::abi_algebra::IAlgebraPool;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::load_algebra::raw_ticks;
use arb::venues::bsc_dex::pools::verify::{Candidate, Verdict};
use arb::venues::bsc_dex::pools::verify_algebra::decide;

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-algebra-{tag}-{}", std::process::id()));
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

/// thena_fusion: в графе, но НЕ исполним (graphable=true, tradable=false, fork=false); требует фабрику и квотер,
/// и НЕ требует Universal Router/Permit2/роутер/читатель тиков.
#[tokio::test]
async fn algebra_family_requirements() {
    let c = ctx("req").await;
    let id = "thena-fusion-bsc";
    let s = Scope::Dex(id.into());
    let a = json!("0x1111111111111111111111111111111111111111");

    let f = families::make(id, "thena_fusion").unwrap();
    assert_eq!(f.proto, Proto::Algebra);
    assert_eq!(
        (f.graphable, f.tradable, f.fork),
        (true, false, false),
        "Algebra: в графе, но без авто-исполнения (решения 80, 81)"
    );

    let req = families::required(&c, f);
    assert_eq!(req, vec!["dex.factory", "dex.quoter"], "req: {req:?}");
    for name in [
        "dex.universal_router",
        "dex.universal_router_version",
        "dex.permit2",
        "dex.router",
        "dex.tick_reader",
    ] {
        assert!(!req.contains(&name), "{name} не нужен Algebra: {req:?}");
    }

    // Только вид — ещё не подключён (нет dex.pool_enabled).
    set(&c, &[("dex.kind", s.clone(), json!("thena_fusion"))]).await;
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

    // Фабрика есть, квотера нет — не подключён.
    set(&c, &[("dex.factory", s.clone(), a.clone())]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("dex.quoter")
    );

    // Заданы оба — подключён.
    set(&c, &[("dex.quoter", s.clone(), a)]).await;
    let f = families::classify(&c, id).unwrap();
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
}

/// Слово ответа ABI из U256 (беззнаковое или уже two's complement). Ошибок нет.
fn word_u(v: U256) -> [u8; 32] {
    v.to_be_bytes::<32>()
}

/// Слово ответа ABI из I256 (знаковое — two's complement с заполнением знаком до 256 бит). Ошибок нет.
fn word_i(v: I256) -> [u8; 32] {
    v.to_be_bytes::<32>()
}

/// Разбор globalState() по ABI на структуре официального контракта (@cryptoalgebra/v1.9-core IAlgebraPoolState.sol):
/// (uint160 price, int24 tick, uint16 fee, uint16 timepointIndex, uint8 cf0, uint8 cf1, bool unlocked) — 7 статических
/// слов (как отдаёт eth_call). Отрицательный тик — two's complement, декодер возвращает его как есть. Комиссия fee —
/// единицы 1e-6 (динамическая).
#[test]
fn algebra_global_state_abi() {
    let price = U256::from(1u8) << 96; // sqrtPriceX96 = 1.0
    let words = [
        word_u(price),
        word_i(I256::try_from(-73_140).unwrap()), // tick
        word_u(U256::from(500u16)),               // fee (0.05% в 1e-6)
        word_u(U256::from(7u16)),                 // timepointIndex
        word_u(U256::ZERO),                       // communityFeeToken0
        word_u(U256::ZERO),                       // communityFeeToken1
        word_u(U256::from(1u8)),                  // unlocked = true
    ];
    let raw: Vec<u8> = words.concat();
    let gs = IAlgebraPool::globalStateCall::abi_decode_returns(&raw).unwrap();
    assert_eq!(U256::from(gs.price), price);
    assert_eq!(gs.tick.as_i32(), -73_140);
    assert_eq!(gs.fee, 500u16);
    assert!(gs.unlocked);
    // Как в load_algebra: цена → U256, комиссия → u32.
    assert_eq!(u32::from(gs.fee), 500u32);
}

/// Разбор ticks(int24) по ABI: нам нужны первые два поля — liquidityTotal (gross, uint128) и liquidityDelta
/// (net, int128); остальные (feeGrowth, cumulatives, initialized) — проходные (8 статических слов). Отрицательный
/// net — two's complement (верхний тик позиции отдаёт −Δ).
#[test]
fn algebra_ticks_abi() {
    let total: u128 = 123_456_789;
    let delta: i128 = -42;
    let words = [
        word_u(U256::from(total)),              // liquidityTotal
        word_i(I256::try_from(delta).unwrap()), // liquidityDelta
        word_u(U256::ZERO),                     // outerFeeGrowth0Token
        word_u(U256::ZERO),                     // outerFeeGrowth1Token
        word_i(I256::ZERO),                     // outerTickCumulative (int56)
        word_u(U256::ZERO),                     // outerSecondsPerLiquidity (uint160)
        word_u(U256::ZERO),                     // outerSecondsSpent (uint32)
        word_u(U256::from(1u8)),                // initialized = true
    ];
    let raw: Vec<u8> = words.concat();
    let t = IAlgebraPool::ticksCall::abi_decode_returns(&raw).unwrap();
    assert_eq!(t.liquidityTotal, total);
    assert_eq!(t.liquidityDelta, delta);
}

/// Восстановление СЫРЫХ тиков из слова битмапа tickTable (raw_ticks): бит b слова w → тик w·256 + b (TickTable.sol,
/// БЕЗ деления на tickSpacing). Проверяем положительное и отрицательное слово.
#[test]
fn algebra_raw_ticks_from_bitmap() {
    // Слово 0, биты 0 и 60 → тики 0 и 60.
    let w0 = (U256::from(1u8) << 0) | (U256::from(1u8) << 60);
    assert_eq!(raw_ticks(0, &[w0]), vec![0, 60]);

    // Слово −1, бит 255 → тик −1·256 + 255 = −1 (сырой тик рядом с нулём ниже него).
    let wm1 = U256::from(1u8) << 255;
    assert_eq!(raw_ticks(-1, &[wm1]), vec![-1]);

    // Два слова подряд: первое = −1 (бит 0 → тик −256), второе = 0 (бит 1 → тик 1).
    let a = U256::from(1u8) << 0;
    let b = U256::from(1u8) << 1;
    assert_eq!(raw_ticks(-1, &[a, b]), vec![-256, 1]);
}

/// Проверка пула фабрикой Algebra (verify_algebra::decide на ABI-ответе poolByPair): свой пул принят, чужой отвергнут.
#[test]
fn algebra_verify_decide() {
    let id = "thena-fusion-bsc";
    let f = families::make(id, "thena_fusion").unwrap();
    let pool: Address = "0x2222222222222222222222222222222222222222"
        .parse()
        .unwrap();
    let coin: Address = "0x00000000000000000000000000000000000000aa"
        .parse()
        .unwrap();
    let quote: Address = "0x00000000000000000000000000000000000000bb"
        .parse()
        .unwrap();
    let cand = Candidate {
        family: f,
        pool: format!("{pool:#x}"),
        coin,
        quote,
    };

    // Фабрика вернула именно этот пул — принят, токены отсортированы (coin < quote).
    let ok = pool.abi_encode();
    let found = decide(&cand, pool, &ok).unwrap();
    assert_eq!(found.reference, PoolRef::Contract(pool));
    assert_eq!((found.token0, found.token1), (coin, quote));
    assert!(found.fee_pips.is_none(), "комиссия Algebra динамическая");

    // Фабрика вернула другой адрес — отклонён.
    let other: Address = "0x3333333333333333333333333333333333333333"
        .parse()
        .unwrap();
    let bad = other.abi_encode();
    assert!(decide(&cand, pool, &bad).is_err());

    // А через полный проход verify это Verdict::Rejected (поведение по ABI-ответу).
    let v = match decide(&cand, pool, &bad) {
        Ok(f) => Verdict::Taken(Box::new(f)),
        Err(w) => Verdict::Rejected(w),
    };
    assert!(matches!(v, Verdict::Rejected(_)));
}
