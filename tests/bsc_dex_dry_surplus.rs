//! Файл: tests/bsc_dex_dry_surplus.rs
//! Что делает: поправка выхода симуляции на излишек пары, созданный пополнением (решение 252, дополнение «dry_fund»,
//! Тигран 10.11: вариант «б»; exec/dry_surplus.rs, exec/dry_surplus_cut.rs) — на фактах сервера 10.11
//! (docs/notes/reflection-facts-2026-11-10.md, раздел 3, eth_simulateV1 на SMARS, блок 132810358):
//! - пара SMARS/WBNB 0xfd94e36c…862c76, token0 = WBNB, монета = token1; резерв SMARS 233129278178920034802337 (раздел 3.2,
//!   «до»), резерв WBNB 1703205554983367227203 (Sync после SMARS_B, раздел 1; резерв SMARS в нём тот же, что в 3.2);
//! - перевод S = 233129278178920034802 между чужими адресами: balanceOf пары 233131995673613624834976, резерв прежний —
//!   прирост излишка 2717494693590032639 (1.16566 % S);
//! - Сим A (продажа как есть): вход пары 212536290799842294991, выход 1547467313996147141 WBNB;
//!   Сим B (перевод, затем продажа): вход 215253817170180385085, выход 1567235296442959896; квотер 1697254529702530534.
//!
//! Комиссия пары — 0,25 % (PancakeSwap V2, math/v2.rs): формула на входах Сим A и Сим B даёт ровно их выходы — так
//! проверено, что резервы и комиссия те, что видел обмен.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{dry_surplus, dry_surplus_cut}, check::simulate, math::v2.

use std::str::FromStr;

use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U64, U256, address};
use rust_decimal::Decimal;

use arb::core::types::PoolKind;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::exec::dry_surplus::{
    Stage, Watch, after_swap, first_hop, grown, push, reserves,
};
use arb::venues::bsc_dex::exec::dry_surplus_cut::{Surplus, line};
use arb::venues::bsc_dex::math::v2;
use arb::venues::bsc_dex::node::abi::IERC20;
use arb::venues::bsc_dex::pools::abi_v2v3::IV2Pair;

const SMARS: Address = address!("c0366a104b429f0806bfa98d0008daa9555b2bed");
const PAIR: Address = address!("fd94e36c1f84dd91df47600697484ba014862c76");
const W2: Address = address!("a9C3CACff416e644Dec51986DFD0E17Cc2132817");

fn n(s: &str) -> U256 {
    U256::from_str(s).unwrap()
}

/// Резервы пары до обмена (монета SMARS, встречный WBNB).
fn res() -> (U256, U256) {
    (n("233129278178920034802337"), n("1703205554983367227203"))
}

fn fee() -> (U256, U256) {
    v2::fee_ratio(Decimal::from_str("0.25").unwrap()).unwrap()
}

/// Ответ getReserves (reserve0 = WBNB, reserve1 = SMARS).
fn reserves_reply() -> CallResult {
    let (coin, wbnb) = res();
    CallResult {
        status: U64::from(1u8),
        return_data: Bytes::from((wbnb, coin, U256::from(1u8)).abi_encode_params()),
        gas_used: U64::ZERO,
        error: None,
        logs: Vec::new(),
    }
}

/// Прирост излишка — ровно рост balanceOf пары от перевода между чужими адресами (раздел 3.2).
#[test]
fn surplus_growth_from_server_transfer() {
    let (coin, _) = res();
    let d = grown((coin, coin), (n("233131995673613624834976"), coin));
    assert_eq!(d, n("2717494693590032639"));
    // Ожидающий излишек, бывший до пополнения (здесь 7 ед.), в прирост не входит.
    let seven = U256::from(7u8);
    let d = grown(
        (coin + seven, coin),
        (n("233131995673613624834976") + seven, coin),
    );
    assert_eq!(d, n("2717494693590032639"));
    // Баланс ниже резерва (монета, уменьшающая балансы) — разность всё равно считается, без переполнения.
    let one = U256::from(1u8);
    assert_eq!(
        grown((coin - one, coin), (coin + one, coin)),
        U256::from(2u8)
    );
}

/// SMARS: на входе Сим B (с переводом перед продажей) формула пары даёт выход Сим B; без прироста — выход Сим A с
/// точностью до второго порядка (своё отражение продажи у Сим B на долю больше: пара держит на прирост больше),
/// налог по выходу 8.8253 % — как у продажи «как есть», а не 7.66 %.
#[test]
fn smars_cut_returns_sale_as_is() {
    let d = n("2717494693590032639");
    let (full, cut) = first_hop(
        n("215253817170180385085"),
        d,
        res(),
        n("1567235296442959896"),
        fee(),
    )
    .unwrap();
    assert_eq!(full, n("1567235296442959896"));
    let sim_a = n("1547467313996147141");
    // Формула на входе Сим A — ровно выход Сим A (резервы и комиссия верны).
    assert_eq!(
        v2::amount_out(n("212536290799842294991"), res().0, res().1, fee()),
        Some(sim_a)
    );
    let gap = if cut > sim_a {
        cut - sim_a
    } else {
        sim_a - cut
    };
    // |поправка − Сим A| / Сим A < 2e-7.
    assert!(
        gap * U256::from(10_000_000u64) < sim_a * U256::from(2u8),
        "gap {gap}"
    );
    let q = Decimal::from_str("1697254529702530534").unwrap();
    let c = Decimal::from_str(&cut.to_string()).unwrap();
    let tax = ((q - c) / q * Decimal::ONE_HUNDRED).round_dp(4);
    assert_eq!(tax, Decimal::from_str("8.8253").unwrap());
}

/// Обычная монета: перевод чужим адресам пулу ничего не добавляет — прирост 0, выход симуляции тот же до единицы
/// (поправка не применяется: exec/dry_surplus_cut.rs пропускает пул с нулевым приростом).
#[test]
fn plain_coin_unchanged() {
    let (coin, _) = res();
    // Баланс и резерв пары до и после пополнения те же (раздел 3.2, «до»).
    let d = grown((coin, coin), (coin, coin));
    assert!(d.is_zero());
    let sim_a = n("1547467313996147141");
    let (full, cut) = first_hop(n("212536290799842294991"), d, res(), sim_a, fee()).unwrap();
    assert_eq!((full, cut), (sim_a, sim_a));
    // Излишек уменьшился (не от пополнения) — прирост 0, ничего не трогаем.
    assert!(grown((n("5"), n("0")), (n("3"), n("0"))).is_zero());
}

/// Формула не сошлась с выходом симуляции (не та комиссия) — отказ, а не «примерная» поправка.
#[test]
fn formula_mismatch_refuses() {
    let wrong = v2::fee_ratio(Decimal::from_str("0.3").unwrap()).unwrap();
    let r = first_hop(
        n("215253817170180385085"),
        n("2717494693590032639"),
        res(),
        n("1567235296442959896"),
        wrong,
    );
    assert!(r.is_err());
}

/// Вызовы замера: до пополнения и после — balanceOf(пары) и getReserves; после обмена — только getReserves; разбор
/// резервов — по стороне монеты (SMARS = token1).
#[test]
fn reads_order_and_reserves_side() {
    let mut ws = vec![Watch {
        leg: 0,
        pool: PAIR,
        kind: PoolKind::V2,
        zero_in: false,
        bal: [None; 2],
        res: [None; 3],
    }];
    let mut calls = Vec::new();
    push(&mut calls, W2, SMARS, &mut ws, Stage::Before);
    push(&mut calls, W2, SMARS, &mut ws, Stage::Funded);
    after_swap(&mut calls, W2, &mut ws);
    assert_eq!(calls.len(), 5);
    assert_eq!(ws[0].bal, [Some(0), Some(2)]);
    assert_eq!(ws[0].res, [Some(1), Some(3), Some(4)]);
    let bal = Bytes::from(IERC20::balanceOfCall { owner: PAIR }.abi_encode());
    assert_eq!((calls[0].to, &calls[0].input), (SMARS, &bal));
    let get = Bytes::from(IV2Pair::getReservesCall {}.abi_encode());
    for i in [1, 3, 4] {
        assert_eq!((calls[i].to, &calls[i].input), (PAIR, &get));
    }
    assert_eq!(reserves(&reserves_reply(), false), Some(res()));
    assert_eq!(reserves(&reserves_reply(), true), Some((res().1, res().0)));
}

/// Строка журнала: прирост в монетах и в % перевода пополнения (1.1657 % у SMARS), уменьшение выхода.
#[test]
fn journal_line() {
    let s = Surplus {
        coins: n("2717494693590032639"),
        funded: n("233129278178920034802"),
        cut: n("19767752019361990"), // Сим B − поправка (smars_cut_returns_sale_as_is)
    };
    // decimals SMARS = 9 (раздел 2), WBNB = 18.
    let t = line(&s, 9, 18, "WBNB");
    assert_eq!(
        t,
        "излишек пары от пополнения: 2717494693.590032639 монет (1.1657 % перевода пополнения), выход уменьшен на \
         0.019768 WBNB"
    );
}
