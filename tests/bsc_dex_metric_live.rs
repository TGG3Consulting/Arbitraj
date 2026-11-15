//! Файл: tests/bsc_dex_metric_live.rs
//! Что проверяет: решение 285 — Metric OMM (metric_omm) на РЕАЛЬНЫХ ответах узла BSC, записанных на блоке 133656733
//! (tests/data/metric_2026-11-15).
//! 1. Проверка пула (verify_metric::decide): ответы isPool и getImmutables четырёх пулов фабрики 0x2a53…8f32 — пара
//!    монета/USDT принята (порядок токенов пула любой: у WBNB/USDT token0 = WBNB), чужая фабрика и чужая монета — отказ;
//!    getImmutables разбирается по ABI SDK v1 (24 слова).
//! 2. Лесенка (load_metric::state + quote/pmm_ladder): точки — ответы виртуального квотера quoteMetric на цепи
//!    (exec_sim.json); в точках выход котировки = ответу пула; base_is_token0 — по паре записи (по возрастанию),
//!    pool_token0 — порядок самого пула.
//! 3. Шаг исполнителя: вид METRIC = 13, data = abi.encode(bool zeroForOne) в порядке токенов пула; только v2; проба на
//!    цепи: исполнитель v2 шагом 13 получил ровно ответ квотера (NVDAB/USDT, WBNB/USDT), неверный флаг — откат.
//! 4. Вид DEX: graphable, без своего пути (tradable = нет), шаг METRIC, topic0 событий — из записанных логов.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::{verify_metric, load_metric, abi_metric, families, exec_callback},
//! exec::executor::kind, quote::pmm_ladder}.

use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::{LadderPoint, PoolState};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{
    abi_metric, events, exec_callback, families, load_metric, verify_metric,
};
use arb::venues::bsc_dex::quote::pmm_ladder;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/metric_2026-11-15/");
const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

fn json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}{name}")).unwrap()).unwrap()
}

fn addr(s: &str) -> Address {
    s.parse().unwrap()
}

fn dec(v: &Value) -> U256 {
    U256::from_str_radix(v.as_str().unwrap(), 10).unwrap()
}

/// Сырой ответ getImmutables из слов записи (пусто = 0).
fn immutables(p: &Value) -> Bytes {
    let mut out = Vec::new();
    for w in p["immutables"].as_array().unwrap() {
        let h = format!("{:0>64}", w.as_str().unwrap());
        out.extend(alloy_primitives::hex::decode(h).unwrap());
    }
    Bytes::from(out)
}

fn word_bool(b: bool) -> Bytes {
    let mut w = vec![0u8; 32];
    w[31] = b as u8;
    Bytes::from(w)
}

#[test]
fn pool_verified_by_factory_and_immutables() {
    let probe = json("probe.json");
    let fam = families::make("metric-bsc", "metric_omm").expect("вид");
    let factory = addr(probe["factory"].as_str().unwrap());
    for p in probe["pools"].as_array().unwrap() {
        let pool = addr(p["pool"].as_str().unwrap());
        let w = p["immutables"].as_array().unwrap();
        let t0 = addr(&format!("0x{:0>40}", w[1].as_str().unwrap()));
        let t1 = addr(&format!("0x{:0>40}", w[2].as_str().unwrap()));
        let coin = if t0 == addr(USDT) { t1 } else { t0 };
        let answers = vec![Ok(word_bool(true)), Ok(immutables(p))];
        let cand = |coin: Address, quote: Address| Candidate {
            family: fam,
            pool: p["pool"].as_str().unwrap().to_string(),
            coin,
            quote,
        };
        let f = verify_metric::decide(&cand(coin, addr(USDT)), pool, factory, &answers)
            .expect("пул принят");
        assert!(f.token0 < f.token1, "пара записи — по возрастанию");
        assert!(verify_metric::decide(&cand(pool, addr(USDT)), pool, factory, &answers).is_err());
        assert!(verify_metric::decide(&cand(coin, addr(USDT)), pool, pool, &answers).is_err());
        let not_listed = vec![Ok(word_bool(false)), Ok(immutables(p))];
        assert!(
            verify_metric::decide(&cand(coin, addr(USDT)), pool, factory, &not_listed).is_err()
        );
    }
}

#[test]
fn ladder_points_are_pool_answers() {
    let sim = json("exec_sim.json");
    let probe = json("probe.json");
    let nv = &probe["pools"][1];
    let w = nv["immutables"].as_array().unwrap();
    let pool_t0 = addr(&format!("0x{:0>40}", w[1].as_str().unwrap()));
    let usdt = addr(USDT);
    let coin = pool_t0;
    let pair = if coin < usdt {
        (coin, usdt)
    } else {
        (usdt, coin)
    };
    let pts: Vec<LadderPoint> = sim["quoteMetric"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["pool"] == nv["pool"])
        .map(|q| LadderPoint {
            amount_in: dec(&q["amountIn"]),
            amount_out: dec(&q["out"]),
        })
        .collect::<Vec<_>>();
    let mut pts = pts;
    pts.sort_by_key(|p| p.amount_in);
    // Цена продажи провайдера (sellPriceX64) пула NVDAB/USDT на блоке 133656733 — записанный ответ getSellAndBuyPrices.
    let sell: u128 = nv["prices"][0].as_str().unwrap().parse().unwrap();
    let state = load_metric::state(pair, coin, pool_t0, Some(sell), pts.clone(), Vec::new());
    let PoolState::OraclePmm(d) = state else {
        panic!("не OraclePmm")
    };
    assert_eq!(d.pool_token0, Some(pool_t0));
    assert_eq!(d.base_is_token0, pair.0 == coin);
    assert!(d.feasible);
    assert_eq!(d.price, U256::from(sell));
    for p in &pts {
        assert_eq!(
            pmm_ladder::out(&d.ladder_quote_in, p.amount_in),
            Some(p.amount_out)
        );
    }
    // Выше последней точки — неисполнимо (выход не придумывается).
    assert_eq!(
        pmm_ladder::out(
            &d.ladder_quote_in,
            pts.last().unwrap().amount_in + U256::from(1u8)
        ),
        None
    );
    // Откат цены провайдера — пул без котировки.
    let PoolState::OraclePmm(dead) =
        load_metric::state(pair, coin, pool_t0, None, Vec::new(), Vec::new())
    else {
        panic!()
    };
    assert!(!dead.feasible && dead.ladder_quote_in.is_empty());
}

#[test]
fn executor_step_and_family() {
    let fam = families::make("metric-bsc", "metric_omm").expect("вид");
    assert!(fam.graphable && !fam.tradable && !fam.fork);
    assert_eq!(fam.executor_kind, Some(ExecKind::Metric));
    assert!(exec_callback::v2_only(ExecKind::Metric));
    let p = Params::Metric { zero_for_one: true };
    assert_eq!(p.kind(), 13);
    assert_eq!(p.data(), word_bool(true));
    assert_eq!(
        Params::Metric {
            zero_for_one: false
        }
        .data(),
        word_bool(false)
    );
    let sim = json("exec_sim.json");
    let q = sim["quoteMetric"].as_array().unwrap();
    let e = sim["execute"].as_array().unwrap();
    assert_eq!(dec(&e[0]["received"]), dec(&q[0]["out"]));
    assert_eq!(dec(&e[1]["received"]), dec(&q[3]["out"]));
    assert!(e[2]["revert"].as_str().unwrap().starts_with("0xbe795921"));
    // Порядок токенов пула не по адресу: у WBNB/USDT token0 = WBNB (больший адрес) — флаг true шага 13.
    let probe = json("probe.json");
    let w = probe["pools"][0]["immutables"].as_array().unwrap();
    assert!(addr(&format!("0x{:0>40}", w[1].as_str().unwrap())) > addr(USDT));
    let topics = events::topics(fam);
    for t in probe["logs_topic0"]["topics"].as_array().unwrap() {
        let t0: alloy_primitives::B256 = t["topic0"].as_str().unwrap().parse().unwrap();
        assert!(topics.contains(&t0));
    }
    assert_eq!(abi_metric::liquidity_topics(), abi_metric::topics());
}
