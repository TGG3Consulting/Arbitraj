//! Файл: tests/bsc_dex_stable_families.rs
//! Что делает: семейства решения 122 F, 3 без сети и без моков — PancakeSwap StableSwap, Lista StableSwap, Saddle
//! (Nerve), WooFi WooPPV2: (1) таблица сопоставления → протокол, вид ядра, в графе, без своего пути, без шага
//! исполнителя: Pancake / Lista — STABLE_U256, Nerve — SADDLE (решение 122 X, стейблсвапы), WooFi — нет; индексы шага
//! из состояния по токену входа, пул на паузе — не исполняется; нужные настройки; (2) свойства общей математики шаблонов
//! (stableswap_v1::Template): A_PRECISION = 100 с A·100 даёт ровно то же, что сырой A (формулы Lista/Saddle сводятся к
//! Curve 2020 точно); рампа A Lista (get_A); проверка цены Lista после обмена (stable_guard): малый обмен в
//! сбалансированном пуле проходит, обмен, уводящий цену за порог, — нет; (3) сверка своей математики с квотером пула
//! на ЖИВОЙ записи узла — ждёт записей tests/data/{pancake_stable,lista_stable,nerve}/responses.json (запрос —
//! tests/README.md), пока записей нет — #[ignore]. WooFi своей математики не имеет (развёрнутый код закрыт).
//! Кто вызывает: cargo test (живые — cargo test --test bsc_dex_stable_families -- --ignored, когда записи есть).
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, math::stableswap_v1, math::stable_guard,
//! pools::load_stable_u::ramp), arb::core::types.

use alloy_primitives::U256;
use serde_json::Value;

use arb::core::types::{PoolKind, StableGuard};
use arb::venues::bsc_dex::math::stable_guard;
use arb::venues::bsc_dex::math::stableswap_v1::{self, Template, curve2020};
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::load_stable_u::ramp;

fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u8).pow(U256::from(18u8))
}

/// Шаблон A_PRECISION (Lista: 255 раундов без отката; Saddle — 256 с откатом).
fn aprec(max_rounds: u16, strict: bool) -> Template {
    Template {
        amp_precision: U256::from(100u8),
        max_rounds,
        strict,
    }
}

/// Виды: протокол, вид ядра StableSwap/OraclePmm, в графе, без своего пути и без шага исполнителя.
#[test]
fn families_flags() {
    for (kind, proto, pk) in [
        (
            "pancake_stableswap",
            Proto::PancakeStable,
            PoolKind::StableSwap,
        ),
        ("lista_stableswap", Proto::ListaStable, PoolKind::StableSwap),
        (
            "saddle_stableswap",
            Proto::SaddleStable,
            PoolKind::StableSwap,
        ),
        ("woofi_v2", Proto::WooPP, PoolKind::OraclePmm),
    ] {
        let f = families::make("x", kind).unwrap();
        assert_eq!((f.proto, f.kind), (proto, pk), "{kind}");
        assert_eq!(
            (f.graphable, f.tradable, f.fork),
            (true, false, false),
            "{kind}"
        );
        // Решение 122 X (стейблсвапы): Pancake / Lista — STABLE_U256, Nerve — SADDLE; WooFi — без шага (нет котировки).
        let want = match kind {
            "pancake_stableswap" | "lista_stableswap" => Some(ExecKind::StableU256),
            "saddle_stableswap" => Some(ExecKind::Saddle),
            _ => None,
        };
        assert_eq!(f.executor_kind, want, "{kind}");
    }
}

/// Окружение на настоящей SQLite без настроек (для текста ошибок entry::params).
async fn ctx(tag: &str) -> arb::venues::bsc_dex::ctx::Ctx {
    use arb::settings::store::Store;
    use arb::storage::{db::Db, journal::Journal, raw_log::RawLog};
    let d = std::env::temp_dir().join(format!("arb-stable-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = tokio::sync::broadcast::channel(64);
    let (jtx, _) = tokio::sync::broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    arb::venues::bsc_dex::ctx::Ctx {
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

/// Шаг исполнителя пула StableSwap: индексы из состояния (i0 — token0, i1 — token1) по токену входа; пул на паузе
/// (halted) — не исполняется. Монеты — USDT / USDC BSC, индексы 2 и 1 (как у пула из трёх монет Nerve).
#[tokio::test]
async fn stable_steps_take_indices_from_state() {
    use alloy_primitives::{Address, address};
    use arb::core::types::{Pool, PoolRef, PoolState, StableData, VenueId};
    use arb::venues::bsc_dex::exec::executor::{entry, kind::Params};
    use arb::venues::bsc_dex::pools::registry::Entry;
    const USDC: Address = address!("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d");
    const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
    let c = ctx("idx").await;
    let make = |kind: &str, halted: bool| {
        let f = families::make("x-stable", kind).unwrap();
        let (t0, t1) = if USDC < USDT {
            (USDC, USDT)
        } else {
            (USDT, USDC)
        };
        let state = StableData {
            balances: vec![e18(1); 3],
            rates: vec![U256::from(1u8); 3],
            amp: U256::from(100u8),
            fee: U256::ZERO,
            amp_precision: U256::from(100u8),
            max_rounds: 256,
            strict: true,
            guard: None,
            halted,
            ng: None,
            i0: 2,
            i1: 1,
            native: None,
        };
        Entry {
            family: f,
            pool: Pool {
                venue: VenueId("bsc_dex".into()),
                family: f.id.into(),
                kind: f.kind,
                reference: PoolRef::Contract(address!(
                    "0x1111111111111111111111111111111111111111"
                )),
                token0: t0,
                token1: t1,
                fee_pips: None,
                tick_spacing: None,
                bin_step: None,
                hooks: None,
                state: PoolState::StableSwap(Box::new(state)),
                block: 1,
                received_at: chrono::Utc::now(),
            },
            ticker: "USDC".into(),
            coin: USDC,
            usdt: USDT,
            key: None,
            ticks: None,
            bins: None,
            synced_block: 1,
            synced_log: None,
            valid: true,
            asleep: false,
        }
    };
    let e = make("saddle_stableswap", false);
    let (t0, t1) = (e.pool.token0, e.pool.token1);
    let (p, _) = entry::params(&c, &e, t0).unwrap();
    assert_eq!(p, Params::Saddle { i: 2, j: 1 });
    let (p, _) = entry::params(&c, &e, t1).unwrap();
    assert_eq!(p, Params::Saddle { i: 1, j: 2 });
    let e = make("lista_stableswap", false);
    let (p, _) = entry::params(&c, &e, t1).unwrap();
    assert_eq!(p, Params::StableU256 { i: 1, j: 2 });
    let e = make("pancake_stableswap", true);
    assert!(entry::params(&c, &e, t0).is_err(), "пул на паузе / kill");
    // Ревью стейблсвапов: монета пары — нативная 0xEeee… (BNB_ADDRESS) — отказ до отправки, у всех трёх видов.
    for k in [
        "pancake_stableswap",
        "lista_stableswap",
        "saddle_stableswap",
    ] {
        let mut e = make(k, false);
        e.pool.token0 = arb::venues::bsc_dex::exec::executor::kind::STABLE_NATIVE;
        let why = entry::params(&c, &e, e.pool.token1).expect_err("нативная монета");
        assert!(why.message.contains("0xEeee"), "{k}: {}", why.message);
    }
    // Решение «122 X (нативный BNB в стейблсвапах)»: нативная монета пула — монета пары (индекс 2 = token0, в паре —
    // WBNB): Pancake / Lista — шаг STABLE_U256_BNB (bnb_in — когда BNB на входе), у Saddle — отказ; нативная монета
    // вне пары (индекс 0) — обычный STABLE_U256.
    let with_native = |k: &str, n: u8| {
        let mut e = make(k, false);
        if let PoolState::StableSwap(d) = &mut e.pool.state {
            d.native = Some(n);
        }
        e
    };
    // Ревью 7b4ea25 (🟡1): сторона BNB шага обязана совпадать с dex.native_wrapped — без неё (или с другой) отказ.
    let e = with_native("lista_stableswap", 2);
    let why = entry::params(&c, &e, t0).expect_err("dex.native_wrapped не задана");
    assert!(
        why.message.contains("dex.native_wrapped"),
        "{}",
        why.message
    );
    c.settings
        .set(
            "dex.native_wrapped",
            arb::settings::scope::Scope::Venue("bsc_dex".into()),
            Some(serde_json::json!(t0.to_string())),
            "тест",
        )
        .await
        .unwrap();
    for k in ["pancake_stableswap", "lista_stableswap"] {
        let e = with_native(k, 2);
        let (p, _) = entry::params(&c, &e, t0).unwrap();
        assert_eq!(
            p,
            Params::StableU256Bnb {
                i: 2,
                j: 1,
                bnb_in: true
            },
            "{k}"
        );
        let (p, _) = entry::params(&c, &e, t1).unwrap();
        assert_eq!(
            p,
            Params::StableU256Bnb {
                i: 1,
                j: 2,
                bnb_in: false
            },
            "{k}"
        );
        let e = with_native(k, 0);
        let (p, _) = entry::params(&c, &e, t1).unwrap();
        assert_eq!(p, Params::StableU256 { i: 1, j: 2 }, "{k}");
    }
    let e = with_native("saddle_stableswap", 2);
    assert!(entry::params(&c, &e, t0).is_err(), "Saddle с BNB");
}

/// A·100 при точности 100 = сырой A при точности 1 (дословно: деления на AP точны), для D и для обмена.
#[test]
fn a_precision_reduces_to_curve2020() {
    let rates = vec![e18(1), e18(1)];
    let balances = vec![e18(1_000_000), e18(700_000)];
    let xp = stableswap_v1::xp(&balances, &rates).unwrap();
    for a in [20u64, 100, 5000] {
        let raw = stableswap_v1::get_d_t(&xp, U256::from(a), &curve2020()).unwrap();
        let prec = stableswap_v1::get_d_t(&xp, U256::from(a * 100), &aprec(255, false)).unwrap();
        assert_eq!(raw, prec, "D, A = {a}");
        let fee = U256::from(1_000_000u64); // 0,01% над 10^10
        let dx = e18(10_000);
        let r = stableswap_v1::exchange_out_t(
            0,
            1,
            dx,
            &balances,
            &rates,
            U256::from(a),
            fee,
            &curve2020(),
        );
        let p = stableswap_v1::exchange_out_t(
            0,
            1,
            dx,
            &balances,
            &rates,
            U256::from(a * 100),
            fee,
            &aprec(256, true),
        );
        assert_eq!(r, p, "обмен, A = {a}");
        assert!(r.unwrap() > U256::ZERO);
    }
}

/// Сбалансированный пул: D = сумма балансов (в 18 знаках) у обоих шаблонов.
#[test]
fn balanced_d_is_sum() {
    let xp = vec![e18(500), e18(500)];
    let d = stableswap_v1::get_d_t(&xp, U256::from(10_000u64), &aprec(256, true)).unwrap();
    let s = e18(1000);
    let diff = if d > s { d - s } else { s - d };
    assert!(diff <= U256::from(2u8), "D {d} ≠ {s}");
}

/// Рампа A (get_A Lista/PancakeSwap): до начала — initial, после конца — future, посередине — линейно.
#[test]
fn ramp_a() {
    let (a0, a1, t0, t1) = (
        U256::from(10_000u64),
        U256::from(20_000u64),
        U256::from(100u8),
        U256::from(200u8),
    );
    assert_eq!(ramp(a0, a1, t0, t1, U256::from(200u8)), Some(a1));
    assert_eq!(
        ramp(a0, a1, t0, t1, U256::from(150u8)),
        Some(U256::from(15_000u64))
    );
    assert_eq!(
        ramp(a1, a0, t0, t1, U256::from(150u8)),
        Some(U256::from(15_000u64))
    );
}

/// Проверка цены Lista: оракул 1 : 1, порог 3% — малый обмен проходит, обмен половины пула — нет.
#[test]
fn guard_small_passes_big_fails() {
    let rates = vec![e18(1), e18(1)];
    let balances = vec![e18(1_000_000), e18(1_000_000)];
    let g = StableGuard {
        admin_fee: U256::from(5_000_000_000u64),
        oracle: vec![e18(1), e18(1)],
        threshold: vec![U256::from(3u8) * U256::from(10u8).pow(U256::from(16u8)); 2],
    };
    let (amp, fee, t) = (
        U256::from(500u64 * 100),
        U256::from(1_000_000u64),
        aprec(255, false),
    );
    assert!(stable_guard::passes(
        0,
        1,
        e18(1_000),
        &balances,
        &rates,
        amp,
        fee,
        &t,
        &g
    ));
    assert!(!stable_guard::passes(
        0,
        1,
        e18(900_000),
        &balances,
        &rates,
        amp,
        fee,
        &t,
        &g
    ));
}

/// Слово hex-ответа eth_call → U256.
fn word(v: &Value) -> U256 {
    let b = alloy_primitives::hex::decode(v.as_str().expect("строка").trim_start_matches("0x"))
        .unwrap();
    U256::from_be_slice(&b)
}

/// Сверка с квотером пула по записи: {block, pools: [{pool, balances: [..], decimals: [..], amp: "<A в точности
/// шаблона>", fee, quotes: [{i, j, dx: "<десятичное>", out: "<result квотера>"}]}]}. Принимает: файл, шаблон.
fn live(file: &str, t: &Template, minus_fee_rounding: bool) {
    let path = format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("запись")).unwrap();
    for p in rec["pools"].as_array().expect("pools") {
        let balances: Vec<U256> = p["balances"].as_array().unwrap().iter().map(word).collect();
        let rates: Vec<U256> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| stableswap_v1::rate_for_decimals(d.as_u64().unwrap() as u8).unwrap())
            .collect();
        let (amp, fee) = (word(&p["amp"]), word(&p["fee"]));
        for q in p["quotes"].as_array().unwrap() {
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let got =
                stableswap_v1::exchange_out_t(i, j, dx, &balances, &rates, amp, fee, t).unwrap();
            let want = word(&q["out"]);
            // get_dy PancakeSwap/Lista снимает комиссию после перевода в единицы монеты — расхождение ≤ 1 единицы
            // при rates ≠ 1e18 (PancakeStableSwapTwoPool.sol L393-L404 против exchange L425-L466).
            let diff = if got > want { got - want } else { want - got };
            let tol = U256::from(u8::from(minus_fee_rounding));
            assert!(
                diff <= tol,
                "пул {} {i}→{j} dx {dx}: {got} ≠ {want}",
                p["pool"]
            );
        }
    }
}

#[test]
#[ignore = "нет записи tests/data/pancake_stable/responses.json — запрос в tests/README.md (решение 122 F, 3)"]
fn pancake_math_matches_live_get_dy() {
    live("pancake_stable/responses.json", &curve2020(), true);
}

#[test]
#[ignore = "нет записи tests/data/lista_stable/responses.json — запрос в tests/README.md (решение 122 F, 3)"]
fn lista_math_matches_live_get_dy() {
    live("lista_stable/responses.json", &aprec(255, false), true);
}

#[test]
#[ignore = "нет записи tests/data/nerve/responses.json — запрос в tests/README.md (решение 122 F, 3)"]
fn nerve_math_matches_live_calculate_swap() {
    live("nerve/responses.json", &aprec(256, true), false);
}
