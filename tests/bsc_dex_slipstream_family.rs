//! Файл: tests/bsc_dex_slipstream_family.rs
//! Что делает: подключение семейства Slipstream (Topaz v3 на BSC; построчный форк Uniswap V3 core; решения 80, 85)
//! без сети и без моков:
//! (1) таблица сопоставления (dex.kind = slipstream): в графе, НЕ исполним (graphable=true, tradable=false, fork=false),
//!   обязательны фабрика и квотер (dex.tick_lens НЕ нужен — тики прямыми геттерами пула);
//! (2) разбор slot0 форка на ОФИЦИАЛЬНОЙ раскладке (6 полей, БЕЗ feeProtocol): sqrtPriceX96/tick встают верно; те же
//!   байты декодером Uniswap V3 (7 полей) не читаются — доказывает, что форку нужен свой ABI slot0;
//! (3) разбор ticks() форка (10 полей Tick.Info с добавками stakedLiquidityNet/rewardGrowthOutsideX128): liquidityGross
//!   и liquidityNet берутся первыми двумя, добавки не сбивают разбор;
//! (4) ключ пула — int24 tickSpacing: verify_calls::registry_call даёт getPool(address,address,int24) с tickSpacing
//!   пула (а не getPool по uint24 fee, как у нашего V3);
//! (5) квотер ISlipstreamQuoterV2 — вход по int24 tickSpacing, выход amountOut первым (как Uni V3 QuoterV2);
//! (6) дискавери по событию CLFactory.PoolCreated(token0, token1, int24 tickSpacing, pool) → кандидат (монета/встречный);
//! (7) восстановление тика из бита битмапа (compressed = floor(tick/tickSpacing), евклидово — как у Uni V3/CL): для
//!   положительных и ОТРИЦАТЕЛЬНЫХ тиков tick = (word·256 + bit)·tickSpacing.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, abi_slipstream, verify_calls, created_log, load_cl::set_ticks),
//! arb::venues::bsc_dex::node::abi_dex (ISlipstreamQuoterV2), arb::settings (SQLite). Селекторы/topic0 — также
//! в модульных тестах abi_slipstream.rs и abi_dex.rs. Живой кросс-чек QuoterV2 на реальном пуле — нет узла, след. фаза.

use alloy::rpc::types::Log;
use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::aliases::{I24, U160};
use alloy_primitives::{Address, I256, U256, address, keccak256};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::abi_dex::ISlipstreamQuoterV2;
use arb::venues::bsc_dex::pools::abi_slipstream::{ICLFactory, ICLPool};
use arb::venues::bsc_dex::pools::abi_v2v3::IUniswapV3Pool;
use arb::venues::bsc_dex::pools::created_log;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::load_cl::set_ticks;
use arb::venues::bsc_dex::pools::verify_calls::registry_call;
use arb::venues::bsc_dex::pools::verify_rules::Read;

const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const COIN: Address = address!("0x0000000000000000000000000000000000000111");

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-slip-{tag}-{}", std::process::id()));
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

/// uint → 32 байта ABI; int → 32 байта с расширением знака (two's complement 256 бит).
fn w_u(v: U256) -> [u8; 32] {
    v.to_be_bytes::<32>()
}
fn w_i(v: i128) -> [u8; 32] {
    I256::try_from(v).unwrap().to_be_bytes::<32>()
}

/// slipstream: вид V3, в графе, НЕ исполним; требует фабрику и квотер (tick_lens не нужен).
#[tokio::test]
async fn slipstream_family_requirements() {
    let c = ctx("req").await;
    let id = "topaz-v3-bsc";
    let s = Scope::Dex(id.into());
    let a = json!("0x1111111111111111111111111111111111111111");

    let f = families::make(id, "slipstream").unwrap();
    assert_eq!(f.proto, Proto::Slipstream);
    assert_eq!(f.kind, arb::core::types::PoolKind::V3, "вид ядра — V3");
    assert_eq!(
        (f.graphable, f.tradable, f.fork),
        (true, false, false),
        "graphable без исполнения (решение 85)"
    );
    assert_eq!(
        families::required(&c, f),
        vec!["dex.factory", "dex.quoter"],
        "фабрика и квотер; dex.tick_lens НЕ нужен, UR/Permit2/роутер — тоже"
    );

    // Только вид — ещё не подключён (нет dex.pool_enabled).
    set(&c, &[("dex.kind", s.clone(), json!("slipstream"))]).await;
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
    // Фабрика есть, но нет квотера — всё ещё не подключён.
    set(&c, &[("dex.factory", s.clone(), a.clone())]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("dex.quoter")
    );
    // Задан и квотер — подключён.
    set(&c, &[("dex.quoter", s.clone(), a)]).await;
    let f = families::classify(&c, id).unwrap();
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
}

/// slot0 форка — 6 полей (без feeProtocol): sqrtPriceX96 и tick декодируются верно. Те же байты декодером Uniswap V3
/// (7 полей) прочитать нельзя — доказывает необходимость своего ABI slot0.
#[test]
fn slipstream_slot0_abi() {
    let sqrt = U256::from(1u128 << 96); // цена 1.0 в Q96
    let tick = -887_220i32; // отрицательный тик (проверка знака int24)
    let mut raw = Vec::new();
    raw.extend_from_slice(&w_u(sqrt)); // sqrtPriceX96 uint160
    raw.extend_from_slice(&w_i(tick as i128)); // tick int24 (знак!)
    raw.extend_from_slice(&w_u(U256::from(7u8))); // observationIndex uint16
    raw.extend_from_slice(&w_u(U256::from(100u16))); // observationCardinality uint16
    raw.extend_from_slice(&w_u(U256::from(100u16))); // observationCardinalityNext uint16
    raw.extend_from_slice(&w_u(U256::from(1u8))); // unlocked bool
    assert_eq!(
        raw.len(),
        6 * 32,
        "slot0 форка — ровно 6 слов (у Uni V3 — 7)"
    );

    let s = ICLPool::slot0Call::abi_decode_returns(&raw).unwrap();
    assert_eq!(s.sqrtPriceX96, U160::from(sqrt));
    assert_eq!(s.tick.as_i32(), tick);
    assert!(s.unlocked);

    // Декодер Uniswap V3 (7 статических полей) не читает 6-словный ответ форка.
    assert!(
        IUniswapV3Pool::slot0Call::abi_decode_returns(&raw).is_err(),
        "6-словный slot0 форка не разбирается ABI Uniswap V3 (7 слов) — нужен свой ABI"
    );
}

/// ticks() форка — 10 полей Tick.Info (добавлены stakedLiquidityNet, rewardGrowthOutsideX128): liquidityGross и
/// liquidityNet берутся первыми двумя, добавки не сбивают разбор.
#[test]
fn slipstream_ticks_abi() {
    let gross = 123_456_789_000u128;
    let net = -987_654_321i128;
    let mut raw = Vec::new();
    raw.extend_from_slice(&w_u(U256::from(gross))); // liquidityGross uint128
    raw.extend_from_slice(&w_i(net)); // liquidityNet int128
    raw.extend_from_slice(&w_i(555i128)); // stakedLiquidityNet int128 (добавка форка)
    raw.extend_from_slice(&w_u(U256::from(10u8))); // feeGrowthOutside0X128
    raw.extend_from_slice(&w_u(U256::from(11u8))); // feeGrowthOutside1X128
    raw.extend_from_slice(&w_u(U256::from(12u8))); // rewardGrowthOutsideX128 (добавка форка)
    raw.extend_from_slice(&w_i(13i128)); // tickCumulativeOutside int56
    raw.extend_from_slice(&w_u(U256::from(14u8))); // secondsPerLiquidityOutsideX128 uint160
    raw.extend_from_slice(&w_u(U256::from(15u8))); // secondsOutside uint32
    raw.extend_from_slice(&w_u(U256::from(1u8))); // initialized bool
    assert_eq!(raw.len(), 10 * 32, "Tick.Info форка — 10 слов");

    let t = ICLPool::ticksCall::abi_decode_returns(&raw).unwrap();
    assert_eq!(t.liquidityGross, gross);
    assert_eq!(t.liquidityNet, net);
    assert_eq!(t.stakedLiquidityNet, 555i128);
}

/// Ключ пула фабрики — int24 tickSpacing: registry_call строит getPool(address,address,int24) с tickSpacing пула,
/// а НЕ getPool по uint24 fee (как у нашего V3).
#[test]
fn slipstream_registry_getpool_by_tickspacing() {
    let f = families::make("topaz-v3-bsc", "slipstream").unwrap();
    let read = Read {
        factory: Address::ZERO,
        token0: COIN,
        token1: USDT,
        fee: Some(500), // комиссия пула — в расчёт getPool НЕ идёт (ключ — tickSpacing)
        spacing: Some(100),
        v2_fee: None,
    };
    let data = registry_call(f, &read);
    assert_eq!(
        data[..4],
        keccak256("getPool(address,address,int24)".as_bytes())[..4],
        "селектор getPool по int24 tickSpacing (а не по uint24 fee)"
    );
    let call = ICLFactory::getPoolCall::abi_decode(&data).unwrap();
    assert_eq!(call.tokenA, COIN);
    assert_eq!(call.tokenB, USDT);
    assert_eq!(
        call.tickSpacing,
        I24::try_from(100).unwrap(),
        "ключ — tickSpacing пула"
    );
}

/// Квотер ISlipstreamQuoterV2: вход — по int24 tickSpacing, selector канонический; выход amountOut декодируется первым
/// (как Uni V3 QuoterV2).
#[test]
fn slipstream_quoter_wire() {
    let params = ISlipstreamQuoterV2::QuoteExactInputSingleParams {
        tokenIn: USDT,
        tokenOut: COIN,
        amountIn: U256::from(1_000_000u64),
        tickSpacing: I24::try_from(200).unwrap(),
        sqrtPriceLimitX96: U160::ZERO,
    };
    let data = ISlipstreamQuoterV2::quoteExactInputSingleCall { params }.abi_encode();
    assert_eq!(
        data[..4],
        keccak256("quoteExactInputSingle((address,address,uint256,int24,uint160))".as_bytes())[..4]
    );
    // tickSpacing переносится в calldata (а не fee).
    let back = ISlipstreamQuoterV2::quoteExactInputSingleCall::abi_decode(&data).unwrap();
    assert_eq!(back.params.tickSpacing, I24::try_from(200).unwrap());

    // Выход квотера (amountOut, sqrtPriceX96After, initializedTicksCrossed, gasEstimate) — amountOut первым.
    let mut ret = Vec::new();
    ret.extend_from_slice(&w_u(U256::from(999u64))); // amountOut
    ret.extend_from_slice(&w_u(U256::from(1u128 << 96))); // sqrtPriceX96After uint160
    ret.extend_from_slice(&w_u(U256::from(3u8))); // initializedTicksCrossed uint32
    ret.extend_from_slice(&w_u(U256::from(120_000u64))); // gasEstimate
    let out = ISlipstreamQuoterV2::quoteExactInputSingleCall::abi_decode_returns(&ret).unwrap();
    assert_eq!(out.amountOut, U256::from(999u64));
}

/// Дискавери по событию: CLFactory.PoolCreated(token0, token1, int24 tickSpacing, pool) → кандидат (монета — не USDT).
#[test]
fn slipstream_poolcreated_candidate() {
    let f = families::make("topaz-v3-bsc", "slipstream").unwrap();
    let pool = address!("0x1000000000000000000000000000000000000001");
    let (t0, t1) = if COIN < USDT {
        (COIN, USDT)
    } else {
        (USDT, COIN)
    };
    let ev = ICLFactory::PoolCreated {
        token0: t0,
        token1: t1,
        tickSpacing: I24::try_from(100).unwrap(),
        pool,
    };
    let data = ev.encode_log_data();
    let raw = json!({
        "address": "0x73DC984D9490286E735548f61dfCCec67Af82ed9",
        "topics": data.topics(),
        "data": data.data,
        "blockNumber": "0x1", "blockHash": format!("{:#066x}", 1),
        "transactionHash": format!("{:#066x}", 2),
        "transactionIndex": "0x0", "logIndex": "0x0", "removed": false
    })
    .to_string();
    let log: Log = serde_json::from_str(&raw).unwrap();
    let c = created_log::candidate(f, &log, USDT).unwrap();
    assert_eq!(c.coin, COIN, "монета — не встречный USDT");
    assert_eq!(c.quote, USDT);
    assert_eq!(c.pool, format!("{pool:#x}"));

    // Пул без USDT в событии — не кандидат (нет встречного токена).
    let other = address!("0x2000000000000000000000000000000000000002");
    let (o0, o1) = if COIN < other {
        (COIN, other)
    } else {
        (other, COIN)
    };
    let ev2 = ICLFactory::PoolCreated {
        token0: o0,
        token1: o1,
        tickSpacing: I24::try_from(50).unwrap(),
        pool,
    };
    let d2 = ev2.encode_log_data();
    let raw2 = json!({
        "address": "0x73DC984D9490286E735548f61dfCCec67Af82ed9",
        "topics": d2.topics(), "data": d2.data,
        "blockNumber": "0x1", "blockHash": format!("{:#066x}", 1),
        "transactionHash": format!("{:#066x}", 2),
        "transactionIndex": "0x0", "logIndex": "0x0", "removed": false
    })
    .to_string();
    let log2: Log = serde_json::from_str(&raw2).unwrap();
    assert!(created_log::candidate(f, &log2, USDT).is_err());
}

/// Восстановление тика из бита битмапа (как Uni V3/CL, load_cl::set_ticks): tick = (word·256 + bit)·tickSpacing,
/// compressed = floor(tick/tickSpacing) евклидово — для положительных и отрицательных тиков.
#[test]
fn slipstream_bitmap_tick_reconstruct() {
    let spacing = 100i32;
    // Слово -1: бит 255 → compressed = -1·256 + 255 = -1 → tick = -100. Слово 0: бит 0 → compressed 0 → tick 0,
    // бит 5 → compressed 5 → tick 500.
    let mut word_neg1 = U256::ZERO;
    word_neg1 |= U256::from(1u8) << 255; // бит 255
    let mut word0 = U256::ZERO;
    word0 |= U256::from(1u8); // бит 0
    word0 |= U256::from(1u8) << 5; // бит 5
    let ticks = set_ticks(-1, &[word_neg1, word0], spacing);
    assert_eq!(
        ticks,
        vec![-100, 0, 500],
        "tick = (word·256+bit)·spacing, знак сохранён"
    );
}
