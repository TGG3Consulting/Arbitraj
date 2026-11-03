//! Файл: tests/bsc_dex_maverick_family.rs
//! Что делает: семейство Maverick V2 (dex.kind = maverick_v2, решение 122 F) без сети: (1) таблица сопоставления →
//! Proto::Maverick, вид ядра TickBin, в графе, без своего пути, шаг MAVERICK исполнителя, нужные адреса; (2) проверка
//! пула по ответам isFactoryPool / tokenA / tokenB / permissionedSwap (verify_maverick::decide); (3) поведение порта
//! цикла обмена (math/maverick_swap.rs — порт команды Maverick, paraswap-dex-lib 4f74b83 maverick-pool-math.ts): откат
//! за пределом тика, точный выход больше резерва, MVP-03M аудита Omniscia (несколько пустых тиков подряд и упор в
//! предел — активный тик откатывается только на ±1), остановка на непрочитанном тике; (4) сверка порта с ответами
//! MaverickV2Quoter.calculateSwap на ЖИВОЙ записи своего узла — ждёт tests/data/maverick_v2/responses.json (что записать
//! — tests/README.md), пока записи нет — #[ignore].
//! Состояния в (3) — входные числа чистой функции (поведение ветвей порта), не подмена ответа площадки.
//! Кто вызывает: cargo test (четвёртый — cargo test --test bsc_dex_maverick_family -- --ignored, когда запись есть).
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, verify_maverick, abi_maverick, math::maverick_swap).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::{PoolKind, TickBinData, TickReserves};
use arb::venues::bsc_dex::math::maverick_swap::swap;
use arb::venues::bsc_dex::pools::abi_maverick::{IMaverickV2Factory as F, IMaverickV2Pool as P};
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::verify_maverick::decide;

/// Монеты BSC — адреса как у BingX (tests/data/indexers_2026-10-24/README.md): CAKE, USDT.
const CAKE: &str = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82";
const USDT: &str = "0x55d398326f99059fF775485246999027B3197955";

fn a(s: &str) -> Address {
    s.parse().unwrap()
}
fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u8).pow(U256::from(18u8))
}

/// maverick_v2: вид ядра TickBin, в графе, без своего пути, шаг MAVERICK.
#[test]
fn maverick_family_flags() {
    let f = families::make("maverick-v2-bsc", "maverick_v2").unwrap();
    assert_eq!(f.proto, Proto::Maverick);
    assert_eq!(f.kind, PoolKind::TickBin);
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
    assert_eq!(f.executor_kind, Some(ExecKind::Maverick));
}

/// Пул фабрики с верной парой — принят; не из фабрики, чужая пара, закрытый обмен — отказ с причиной.
#[test]
fn verify_decides_by_factory_and_tokens() {
    let f = families::make("maverick-v2-bsc", "maverick_v2").unwrap();
    let pool = a("0x2222222222222222222222222222222222222222");
    let c = Candidate {
        family: f,
        pool: format!("{pool:#x}"),
        coin: a(CAKE),
        quote: a(USDT),
    };
    let (t0, t1) = if a(CAKE) < a(USDT) {
        (a(CAKE), a(USDT))
    } else {
        (a(USDT), a(CAKE))
    };
    let ans = |known: bool, ta: Address, tb: Address, closed: bool| -> Vec<Result<Bytes, String>> {
        vec![
            Ok(F::isFactoryPoolCall::abi_encode_returns(&known).into()),
            Ok(P::tokenACall::abi_encode_returns(&ta).into()),
            Ok(P::tokenBCall::abi_encode_returns(&tb).into()),
            Ok(P::permissionedSwapCall::abi_encode_returns(&closed).into()),
        ]
    };
    let found = decide(&c, pool, &ans(true, t0, t1, false)).expect("пул фабрики");
    assert_eq!((found.token0, found.token1), (t0, t1));
    assert!(
        decide(&c, pool, &ans(false, t0, t1, false))
            .unwrap_err()
            .contains("isFactoryPool")
    );
    assert!(decide(&c, pool, &ans(true, t1, t0, false)).is_err());
    assert!(
        decide(&c, pool, &ans(true, t0, t1, true))
            .unwrap_err()
            .contains("permissionedSwap")
    );
}

/// Пул: активный тик 0 с резервом только tokenB (вход tokenA его выкупает), прочитан диапазон [-10, 10].
fn one_tick_pool() -> TickBinData {
    TickBinData {
        active_tick: 0,
        reserve0: e18(1),
        reserve1: e18(1000),
        protocol_fee_d3: 0,
        tick_spacing: U256::from(1u8),
        fee0_in: U256::from(10u8).pow(U256::from(15u8)),
        fee1_in: U256::from(10u8).pow(U256::from(15u8)),
        scale0: U256::from(1u8),
        scale1: U256::from(1u8),
        tick_lo: -10,
        tick_hi: 10,
        ticks: vec![TickReserves {
            tick: 0,
            reserve0: 0,
            reserve1: 1000 * 10u128.pow(18),
            total_supply: 1,
        }],
    }
}

/// Предел тика позади активного — откат (Beyond Swap Limit); точный выход больше резерва — откат.
#[test]
fn reverts_like_pool() {
    let p = one_tick_pool();
    assert!(swap(&p, e18(1), true, false, -1).is_none());
    assert!(swap(&p, e18(1), false, false, 1).is_none());
    assert!(swap(&p, e18(1001), true, true, i32::MAX).is_none());
}

/// Малый вход: весь в одном тике, выход не больше резерва тика, расчёт полный.
#[test]
fn small_swap_in_one_tick() {
    let p = one_tick_pool();
    let r = swap(&p, e18(1), true, false, i32::MAX).unwrap();
    assert!(r.complete && !r.unread);
    assert_eq!(r.amount_in, e18(1));
    assert!(r.amount_out > U256::ZERO && r.amount_out <= e18(1000));
    assert_eq!(r.end_tick, 0);
}

/// MVP-03M: тик 0 выработан (активный → 1), тики 1..=5 пусты, предел 5 — цикл доходит до тика 6, упирается в предел,
/// и активный тик откатывается на −1 от 1, то есть в 0 (а не в 5). Расчёт неполный, но не «непрочитан».
#[test]
fn mvp_03m_rollback_only_one_tick() {
    let p = one_tick_pool();
    let r = swap(&p, e18(1_000_000), true, false, 5).unwrap();
    assert!(!r.complete && !r.unread);
    assert_eq!(r.end_tick, 0);
}

/// Без предела: тики за прочитанным диапазоном неизвестны — остановка, итог частичный («непрочитан»).
#[test]
fn stops_at_unread_tick() {
    let mut p = one_tick_pool();
    p.tick_hi = 2;
    let r = swap(&p, e18(1_000_000), true, false, i32::MAX).unwrap();
    assert!(!r.complete && r.unread);
    assert!(r.amount_in < e18(1_000_000));
}

/// Сверка порта с MaverickV2Quoter.calculateSwap на записи своего узла (один блок; формат — tests/README.md):
/// расхождение не больше 1 единицы токена (точность, заявленная портом @yldfi/maverick-v2-math — ±1 wei).
#[test]
#[ignore = "нет записи tests/data/maverick_v2/responses.json — запросы в tests/README.md"]
fn maverick_port_matches_live_quoter() {
    let raw = std::fs::read_to_string("tests/data/maverick_v2/responses.json").expect("запись");
    let v: Value = serde_json::from_str(&raw).unwrap();
    let n = |x: &Value| -> U256 { x.as_str().unwrap().parse().unwrap() };
    let i = |x: &Value| -> i32 { x.as_i64().unwrap() as i32 };
    for p in v["pools"].as_array().unwrap() {
        let ticks = p["ticks"].as_array().unwrap().iter().map(|t| TickReserves {
            tick: i(&t["tick"]),
            reserve0: t["reserveA"].as_str().unwrap().parse().unwrap(),
            reserve1: t["reserveB"].as_str().unwrap().parse().unwrap(),
            total_supply: t["totalSupply"].as_str().unwrap().parse().unwrap(),
        });
        let active = i(&p["activeTick"]);
        let r = i(&p["tickRadius"]);
        let st = TickBinData {
            active_tick: active,
            reserve0: n(&p["reserveA"]),
            reserve1: n(&p["reserveB"]),
            protocol_fee_d3: p["protocolFeeRatioD3"].as_u64().unwrap() as u8,
            tick_spacing: n(&p["tickSpacing"]),
            fee0_in: n(&p["feeAIn"]),
            fee1_in: n(&p["feeBIn"]),
            scale0: n(&p["tokenAScale"]),
            scale1: n(&p["tokenBScale"]),
            tick_lo: active - r,
            tick_hi: active + r,
            ticks: ticks.collect(),
        };
        for q in p["quotes"].as_array().unwrap() {
            let a_in = q["tokenAIn"].as_bool().unwrap();
            let limit = if a_in { i32::MAX } else { i32::MIN };
            let local = swap(&st, n(&q["amount"]), a_in, false, limit).expect("расчёт");
            assert!(
                local.complete,
                "{}: расчёт неполный — увеличить tickRadius записи",
                p["pool"]
            );
            let remote = n(&q["amountOut"]);
            let d = if local.amount_out > remote {
                local.amount_out - remote
            } else {
                remote - local.amount_out
            };
            assert!(
                d <= U256::from(1u8),
                "{} {q}: порт {} против квотера {remote}",
                p["pool"],
                local.amount_out
            );
        }
    }
}
