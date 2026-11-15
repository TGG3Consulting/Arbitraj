//! Файл: tests/bsc_dex_multi_pair_golden_stable.rs
//! Что делает: эталонный снимок многомонетных видов БЕЗ настройки dex.multi_pair (решение 295, шаг 0: «не задана —
//! 1 в 1 как сейчас») на ЖИВЫХ записях узла BSC: состояние пары, собранное разбором загрузки (load_saddle — Nerve 3pool,
//! load_curve — Ellipsis 3pool; каждая из трёх пар), итог проверки пула (verify_balancer::decide — пулы ACSI
//! balancer_acsi_2026-11-15, verify_curve_ng::decide — пулы curve_ng_2026-11-14) по всем упорядоченным парам монет пула.
//! Снимок записан кодом ДО правки; после каждого шага — тот же текст байт в байт (tests/golden_common).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_golden_stable.
//! От чего зависит: tests/multi_common, tests/golden_common, arb::venues::bsc_dex::pools.

mod golden_common;
mod multi_common;

use std::str::FromStr;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::venues::bsc_dex::pools::verify::Candidate;
use arb::venues::bsc_dex::pools::{families, load_curve, load_saddle, verify_balancer, verify_curve_ng};

use golden_common::{check, sorted};
use multi_common::{PAIRS3, bad, decimals, loader_answers, pair, rec};

/// Состояния трёх пар Nerve и Ellipsis, собранные разбором загрузки. Ошибок нет (паника теста).
fn stable_states() -> String {
    let mut out = Vec::new();
    let nerve = rec("nerve_2026-11-15", "probe.json");
    let pool = Address::from_str(nerve["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_saddle::calls(pool);
    let (head, t) = loader_answers(
        &nerve,
        &calls,
        tail,
        ["nerve getToken", "nerve getTokenBalance"],
        &["nerve getAPrecise()", "nerve swapStorage()", "nerve paused()"],
    );
    for (a, b) in PAIRS3 {
        let p = load_saddle::parse(&head, &t, pair("nerve_2026-11-15", a, b), &bad).unwrap();
        let dec = decimals(&nerve, "nerve", "getToken", p.coins.len());
        out.push(format!("nerve {a}/{b} {:?}", load_saddle::build(p, &dec, &bad).unwrap()));
    }
    let ell = rec("ellipsis_2026-11-15", "probe.json");
    let pool = Address::from_str(ell["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_curve::calls(pool);
    let (head, t) = loader_answers(
        &ell,
        &calls,
        tail,
        ["ellipsis coins", "ellipsis balances"],
        &["ellipsis A()", "ellipsis fee()"],
    );
    for (a, b) in PAIRS3 {
        let (coins, bal, amp, fee, i0, i1) =
            load_curve::parse(&head, &t, pair("ellipsis_2026-11-15", a, b), &bad).unwrap();
        let dec = decimals(&ell, "ellipsis", "coins", coins.len());
        let st = load_curve::build(bal, amp, fee, (i0, i1), &dec, &bad).unwrap();
        out.push(format!("ellipsis {a}/{b} {st:?}"));
    }
    sorted(out)
}

/// Адрес из строки JSON. Паника — не адрес.
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Итог verify_balancer::decide по всем упорядоченным парам токенов каждого пула записи ACSI. Ошибок нет.
fn balancer() -> String {
    let probe = rec("balancer_acsi_2026-11-15", "probe.json");
    let fam = families::make("acsi-finance", "balancer_v2").expect("вид");
    let vault = addr(&probe["vault"]["address"]);
    let wf = addr(&probe["factories"]["weighted"]["address"]);
    let sf = addr(&probe["factories"]["stable"]["address"]);
    let dec = |v: &Value| U256::from_str_radix(v.as_str().unwrap(), 10).unwrap();
    let mut out = Vec::new();
    for p in probe["pools"].as_array().unwrap() {
        let pool = addr(&p["pool"]);
        let tokens: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
        let bal: Vec<U256> = p["balances"].as_array().unwrap().iter().map(dec).collect();
        let ret = |b: bool| Ok::<Bytes, String>(Bytes::from(b.abi_encode()));
        let answers = vec![
            Ok(Bytes::from((addr(&p["getPool"][0]), dec(&p["getPool"][1])).abi_encode_params())),
            Ok(Bytes::from((tokens.clone(), bal, dec(&p["lastChangeBlock"])).abi_encode_params())),
            ret(p["isPoolFromFactory"]["weighted"] == "1"),
            ret(p["isPoolFromFactory"]["stable"] == "1"),
        ];
        for a in &tokens {
            for b in &tokens {
                if a == b {
                    continue;
                }
                let cand = Candidate { family: fam, pool: p["pool"].as_str().unwrap().into(), coin: *a, quote: *b };
                let r = verify_balancer::decide(&cand, (pool, vault, wf, Some(sf)), &answers);
                out.push(format!("{pool:#x} {a:#x}/{b:#x} {r:?}"));
            }
        }
    }
    sorted(out)
}

/// Итог verify_curve_ng::decide по всем упорядоченным парам монет каждого пула записи Curve NG. Ошибок нет.
fn curve_ng() -> String {
    let probe = rec("curve_ng_2026-11-14", "probe.json");
    let fam = families::make("curve-bsc", "curve_stableswap_ng").expect("вид");
    let hex = |s: &str| Bytes::from(alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap());
    let v7 = vec!["v7.0.0".to_string()];
    let mut out = Vec::new();
    for p in probe["pools"].as_array().unwrap() {
        let pool = addr(&p["pool"]);
        let coins: Vec<Address> = p["coins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Address::from_slice(&hex(c.as_str().unwrap())[12..32]))
            .collect();
        let f = &p["factory"];
        let answers: Vec<Result<Bytes, String>> = vec![
            Ok(hex(f["get_coins"].as_str().unwrap())),
            Ok(hex(f["is_meta"].as_str().unwrap())),
            Ok(hex(p["calls"]["version"].as_str().unwrap())),
        ];
        for a in &coins {
            for b in &coins {
                if a == b {
                    continue;
                }
                let cand = Candidate { family: fam, pool: p["pool"].as_str().unwrap().into(), coin: *a, quote: *b };
                let r = verify_curve_ng::decide(&cand, pool, &v7, &answers);
                out.push(format!("{pool:#x} {a:#x}/{b:#x} {r:?}"));
            }
        }
    }
    sorted(out)
}

#[test]
fn multi_coin_kinds_without_setting_unchanged() {
    let mut text = String::from("## stable states\n");
    text += &stable_states();
    text += "## balancer verify\n";
    text += &balancer();
    text += "## curve_ng verify\n";
    text += &curve_ng();
    check("stable_2026-11-15", &text);
}
