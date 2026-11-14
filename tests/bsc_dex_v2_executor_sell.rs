//! Файл: tests/bsc_dex_v2_executor_sell.rs
//! Что делает: ревью решения 280 — сверка пары V2-форка без роутера (v2_fork_executor) на РЕАЛЬНЫХ симуляциях
//! eth_simulateV1 (блок 133628447, tests/data/v2_executor_balance_2026-11-15), без сети и без моков:
//! (1) запрос: вызовы quote/v2_pair_calls.rs build побайтно = отправленным (у продажи dao_swap — целиком, у остальных
//!   сторон — SHA-256 строк «from,to,input»);
//! (2) покупка у шести DEX и ПРОДАЖА у двух (dao_swap — монета 0xa7266989… с налогом на перевод из пары; orangedx — без
//!   налога): обмен прошёл, выход сети = math/v2 на запрошенную сумму, «пара отдала» = формула по резервам после sync на
//!   дошедшее (в пределах допуска 0 %); на 1 пипс ниже — «пара отклонила обмен (K)»;
//! (4) наша комиссия ниже реальной на 100 пипсов при допуске 0 — «пара отдала меньше формулы» (Underpaid).
//! Прежняя раскладка и классификация откатов — tests/bsc_dex_v2_executor_revert.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::{v2_pair_calls, v2_pair_quote, v2_pair_sim}, math::v2, check::simulate},
//! sha2.

use alloy_primitives::{Address, Bytes, U256, hex};
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::{Digest, Sha256};

use arb::venues::bsc_dex::check::simulate::{Call, CallResult};
use arb::venues::bsc_dex::math::v2;
use arb::venues::bsc_dex::quote::v2_pair_calls;
use arb::venues::bsc_dex::quote::v2_pair_quote::{executor, owner};
use arb::venues::bsc_dex::quote::v2_pair_sim::{Sim, judge};

const DATA: &str = "tests/data/v2_executor_balance_2026-11-15/sims.json";

fn rec() -> Value {
    serde_json::from_str(&std::fs::read_to_string(DATA).unwrap()).unwrap()
}

fn a(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn results(v: &Value) -> Vec<CallResult> {
    serde_json::from_value(v.clone()).unwrap()
}

fn ratio(p: u32) -> (U256, U256) {
    (U256::from(1_000_000 - p), U256::from(1_000_000u32))
}

/// Резервы (вход, выход) из ответа getReserves.
fn res(c: &CallResult, zero_in: bool) -> (U256, U256) {
    let h = c.return_data.to_vec();
    let (x, y) = (
        U256::from_be_slice(&h[0..32]),
        U256::from_be_slice(&h[32..64]),
    );
    if zero_in { (x, y) } else { (y, x) }
}

/// SHA-256 строк «from,to,input\n» (адреса и данные — hex в нижнем регистре, как в записи).
fn digest(calls: &[Call]) -> String {
    let s: String = calls
        .iter()
        .map(|c| {
            let (f, t) = (hex::encode_prefixed(c.from), hex::encode_prefixed(c.to));
            format!("{f},{t},{}\n", hex::encode_prefixed(&c.input))
        })
        .collect();
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Стороны записи: (DEX, сторона, данные DEX, данные стороны).
fn sides(r: &Value) -> Vec<(String, &'static str, Value, Value)> {
    let mut out = Vec::new();
    for (id, d) in r["dex"].as_object().unwrap() {
        for s in ["buy", "sell"] {
            if !d[s].is_null() {
                out.push((id.clone(), s, d.clone(), d[s].clone()));
            }
        }
    }
    out
}

/// Вызовы build для стороны записи.
fn built(d: &Value, s: &Value) -> Vec<Call> {
    let amount: U256 = s["amount_in"].as_str().unwrap().parse().unwrap();
    let fee = d["fee_pips"].as_u64().unwrap() as u32;
    let tokens = (a(&s["token_in"]), a(&s["token_out"]));
    v2_pair_calls::build(a(&d["pair"]), tokens, fee, amount, (executor(), owner())).unwrap()
}

#[test]
fn recorded_requests_equal_build() {
    let r = rec();
    assert_eq!((a(&r["executor"]), a(&r["owner"])), (executor(), owner()));
    let all = sides(&r);
    assert_eq!(all.len(), 8, "6 покупок + 2 продажи");
    for (id, side, d, s) in &all {
        let b = built(d, s);
        assert_eq!(
            digest(&b),
            s["calls_sha256"].as_str().unwrap(),
            "{id} {side}"
        );
    }
    let s = &r["dex"]["dao_swap"]["sell"];
    let want: Vec<(Address, Address, Bytes)> = s["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                a(&c["from"]),
                a(&c["to"]),
                c["input"].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect();
    let got: Vec<_> = built(&r["dex"]["dao_swap"], s)
        .into_iter()
        .map(|c| (c.from, c.to, c.input))
        .collect();
    assert_eq!(got, want, "продажа dao_swap — побайтно");
}

#[test]
fn recorded_sides_pass_and_k_below() {
    let r = rec();
    for (id, side, d, s) in sides(&r) {
        let fee = d["fee_pips"].as_u64().unwrap() as u32;
        let amount: U256 = s["amount_in"].as_str().unwrap().parse().unwrap();
        let zero_in = s["token_in"] == d["token0"];
        let ok = results(&s["ok"]);
        let Sim::Ok {
            remote,
            got,
            arrived,
            paid,
            expected,
            received,
        } = judge(&ok, zero_in, amount, ratio(fee), Decimal::ZERO)
        else {
            panic!("{id} {side}: обмен с комиссией {fee} пипсов должен пройти");
        };
        let (ri, ro) = res(&ok[0], zero_in);
        let pct = Decimal::from(fee) / Decimal::from(10_000u32);
        let f = v2::fee_ratio(pct).unwrap();
        assert_eq!(
            remote,
            v2::amount_out(amount, ri, ro, f).unwrap(),
            "{id} {side}"
        );
        let (si, so) = res(&ok[3], zero_in);
        assert_eq!(
            expected,
            v2::amount_out(arrived, si, so, f).unwrap(),
            "{id} {side}"
        );
        assert_eq!(paid, expected, "{id} {side}: пара отдала ровно формулу");
        let taxed_sell = id == "dao_swap" && side == "sell";
        if taxed_sell {
            // Налог на перевод из пары: исполнителю пришло меньше, до пары дошло ещё меньше (второй перевод).
            assert!(got < amount && arrived < got, "{got} {arrived} {amount}");
            assert_eq!(received, paid, "выход — BUSD без налога");
        } else {
            assert_eq!((got, arrived), (amount, amount), "{id} {side}");
        }
        if id == "dao_swap" && side == "buy" {
            assert!(received < paid, "у монеты выхода налог перевода из пары");
        } else if !taxed_sell {
            assert_eq!(received, paid, "{id} {side}");
        }
        assert!(s["below_1pip_steps_0_4_same"].as_bool().unwrap());
        let mut bad = ok[..5].to_vec();
        bad.push(serde_json::from_value(s["below_1pip_last"].clone()).unwrap());
        bad.push(ok[6].clone()); // шаг 6 при откате шага 5 не разбирается
        match judge(&bad, zero_in, amount, ratio(fee - 1), Decimal::ZERO) {
            Sim::Rejected(why) => assert!(why.ends_with(": K"), "{id} {side}: {why}"),
            o => panic!("{id} {side}: на 1 пипс ниже ожидался откат K, а {o:?}"),
        }
        match judge(&ok, zero_in, amount, ratio(fee - 100), Decimal::ZERO) {
            Sim::Underpaid(why) => assert!(why.contains("пара отдала"), "{why}"),
            o => panic!("{id} {side}: при комиссии ниже реальной ожидалось Underpaid, а {o:?}"),
        }
    }
}
