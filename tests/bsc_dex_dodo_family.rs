//! Файл: tests/bsc_dex_dodo_family.rs
//! Что делает: подключение семейства DODO PMM (DVM/DPP/DSP, решения 80, 86b) без сети и без моков:
//! - таблица сопоставления (dex.kind = dodo_dvm/dodo_dpp/dodo_dsp), набор обязательных настроек (только фабрика —
//!   БЕЗ Universal Router/Permit2/роутера/квотера: квотер у DODO — сам пул, исполнения нет до 86c);
//! - разбор ответа getPMMStateForCall по ABI (IDodoPool) на ОФИЦИАЛЬНЫХ числовых векторах DODOEX/contractV2 →
//!   поля i/K/B/Q/B0/Q0/R встают на свои места, а чистый выход с нулевыми комиссиями совпадает с on-chain
//!   балансом официального теста (как в 86a) — связывает ЧТЕНИЕ+КОМИССИИ+сверку с официальным значением.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, abi_dodo, load_dodo, math::swap_dodo), arb::settings (SQLite).
//! Математика DODO на официальных значениях — tests/bsc_dex_dodo_math.rs (86a).

use alloy::sol_types::SolCall;
use alloy_primitives::U256;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::math::swap_dodo::{PmmState, RState, adjusted_target, sell_base_token};
use arb::venues::bsc_dex::pools::abi_dodo::IDodoPool;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::load_dodo::{net_after_fees, rstate};

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-dodo-{tag}-{}", std::process::id()));
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

/// dodo_dvm/dpp/dsp: в графе, но НЕ исполнимы (graphable=true, tradable=false, fork=false); требует только фабрику.
#[tokio::test]
async fn dodo_family_requirements() {
    let c = ctx("req").await;
    let id = "dodo-bsc";
    let s = Scope::Dex(id.into());
    let a = json!("0x1111111111111111111111111111111111111111");

    for (kind, proto) in [
        ("dodo_dvm", Proto::DodoDvm),
        ("dodo_dpp", Proto::DodoDpp),
        ("dodo_dsp", Proto::DodoDsp),
    ] {
        let f = families::make(id, kind).unwrap();
        assert_eq!(f.proto, proto, "{kind}");
        assert_eq!(
            (f.graphable, f.tradable, f.fork),
            (true, false, false),
            "{kind}: graphable без исполнения (решение 86b)"
        );
        assert!(f.proto.is_dodo());
        let req = families::required(&c, f);
        assert_eq!(req, vec!["dex.factory"], "{kind}: только фабрика");
    }

    // Только вид — ещё не подключён (нет dex.pool_enabled).
    set(&c, &[("dex.kind", s.clone(), json!("dodo_dvm"))]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("не задана dex.pool_enabled"),
        "{}",
        families::classify(&c, id).unwrap_err()
    );

    // Включили, но без фабрики — не подключён.
    set(&c, &[("dex.pool_enabled", s.clone(), json!(true))]).await;
    assert!(
        families::classify(&c, id)
            .unwrap_err()
            .contains("dex.factory"),
        "{}",
        families::classify(&c, id).unwrap_err()
    );

    // Задана фабрика — подключён.
    set(&c, &[("dex.factory", s.clone(), a)]).await;
    let f = families::classify(&c, id).unwrap();
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
}

/// Разбор ответа getPMMStateForCall по ABI на ОФИЦИАЛЬНОМ векторе DVM (test/DVM/trader.test.ts, i=100e18, K=0.1e18,
/// резервы после первого трейда B=8047378541243650163, Q=200e18; getPMMState задаёт Q0=0, R=ABOVE_ONE и решает B0).
/// Кодируем 7 слов как контракт, декодируем нашим IDodoPool, собираем PmmState, считаем чистый выход sellBase(1) с
/// нулевыми комиссиями — должен совпасть с on-chain receiveQuote 103734814802481693100 (86a).
#[test]
fn dodo_pmm_state_abi_and_net_official() {
    let one = U256::from(10u8).pow(U256::from(18u8));
    let i = U256::from(100u64) * one;
    let k = one / U256::from(10u8);
    let b = "8047378541243650163".parse::<U256>().unwrap();
    let q = U256::from(200u64) * one;

    // B0 как adjustedTarget контракта (на чтении контракт возвращает уже решённый B0).
    let mut seed = PmmState {
        i,
        k,
        b,
        q,
        b0: U256::ZERO,
        q0: U256::ZERO,
        r: RState::AboveOne,
    };
    adjusted_target(&mut seed).unwrap();

    // Кодируем ответ getPMMStateForCall() = 7 слов (i,K,B,Q,B0,Q0,R) и декодируем своим ABI.
    let words = [
        seed.i,
        seed.k,
        seed.b,
        seed.q,
        seed.b0,
        seed.q0,
        U256::from(1u8),
    ];
    let mut raw = Vec::new();
    for w in words {
        raw.extend_from_slice(&w.to_be_bytes::<32>());
    }
    let decoded = IDodoPool::getPMMStateForCallCall::abi_decode_returns(&raw).unwrap();
    assert_eq!(decoded.i, i);
    assert_eq!(decoded.K, k);
    assert_eq!(decoded.B, b);
    assert_eq!(decoded.Q, q);
    assert_eq!(decoded.B0, seed.b0);
    assert_eq!(decoded.R, U256::from(1u8));
    assert_eq!(rstate(decoded.R.to::<u8>()), Some(RState::AboveOne));

    // Собираем PmmState из декодированного и считаем чистый выход sellBase(1), комиссии нулевые → == on-chain.
    let state = PmmState {
        i: decoded.i,
        k: decoded.K,
        b: decoded.B,
        q: decoded.Q,
        b0: decoded.B0,
        q0: decoded.Q0,
        r: rstate(decoded.R.to::<u8>()).unwrap(),
    };
    let (gross, _) = sell_base_token(&state, one).unwrap();
    let net = net_after_fees(gross, U256::ZERO, U256::ZERO).unwrap();
    assert_eq!(net, "103734814802481693100".parse::<U256>().unwrap());
}

/// net_after_fees повторяет формулу querySellBase (DVMTrader.sol): receive = gross − mulFloor(gross,lp) −
/// mulFloor(gross,mt). Проверка на простом gross и комиссиях lp=0.3%, mt=0.1% (фикс-точка 18 знаков).
#[test]
fn dodo_net_after_fees_formula() {
    let one = U256::from(10u8).pow(U256::from(18u8));
    let gross = U256::from(1_000_000u64) * one; // 1e24
    let lp = one / U256::from(1000u16) * U256::from(3u8); // 0.003e18 = 0.3%
    let mt = one / U256::from(1000u16); // 0.001e18 = 0.1%
    // Ожидание: gross − gross*0.003 − gross*0.001 = gross*0.996.
    let want = gross * U256::from(996u16) / U256::from(1000u16);
    assert_eq!(net_after_fees(gross, lp, mt), Some(want));
}
