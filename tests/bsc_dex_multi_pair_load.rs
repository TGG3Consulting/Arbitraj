//! Файл: tests/bsc_dex_multi_pair_load.rs
//! Что делает: решение 295, тест (б) — состояние многомонетного пула читается один раз на контракт:
//! - группы пачечного чтения load_site::groups — пары `PoolRef::Multi` одного контракта — одна группа (одно чтение
//!   load_site::states), ссылки Contract в группы не входят (их чтение прежнее);
//! - из ОДНОГО прочитанного пула (load_saddle::Raw / load_curve::Raw — ответы и decimals, без пары) собираются
//!   состояния всех трёх пар; каждое = состоянию прежнего разбора загрузки этой пары (parse → build, как load), балансы,
//!   A, fee и множители у пар одинаковы, различаются только индексы пары (calls / read пары не принимают).
//!   Данные — ЖИВЫЕ записи nerve_2026-11-15 и ellipsis_2026-11-15 (3pool).
//!
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_load.
//! От чего зависит: tests/multi_common, arb::venues::bsc_dex::pools::{load_site, load_batch, load_saddle, load_curve}.

mod multi_common;

use std::str::FromStr;

use alloy_primitives::Address;

use arb::core::types::{PoolRef, PoolState};
use arb::venues::bsc_dex::pools::load_batch::Want;
use arb::venues::bsc_dex::pools::{families, load_curve, load_saddle, load_site};

use multi_common::{PAIRS3, bad, decimals, loader_answers, pair, rec};

/// Индексы и прочее состояния StableSwap: (i0, i1) и состояние без индексов (для сравнения пар). Ошибок нет.
fn split(s: &PoolState) -> ((u8, u8), String) {
    let PoolState::StableSwap(d) = s else { panic!("не StableSwap") };
    let mut x = (**d).clone();
    let ix = (x.i0, x.i1);
    (x.i0, x.i1) = (0, 0);
    (ix, format!("{x:?}"))
}

#[test]
fn groups_by_contract_only_multi() {
    let probe = rec("nerve_2026-11-15", "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let ell = Address::from_str(rec("ellipsis_2026-11-15", "probe.json")["pool"].as_str().unwrap()).unwrap();
    let f = families::make("nerve", "saddle_stableswap").unwrap();
    let g = families::make("ellipsis", "curve_stableswap").unwrap();
    let refs: Vec<(&'static families::Family, PoolRef)> = PAIRS3
        .iter()
        .map(|(a, b)| {
            let (t0, t1) = pair("nerve_2026-11-15", a, b);
            (f, PoolRef::Multi { contract: pool, token0: t0, token1: t1 })
        })
        .chain([(g, PoolRef::Contract(ell))])
        .chain([(g, {
            let (t0, t1) = pair("ellipsis_2026-11-15", "BUSD", "USDT");
            PoolRef::Multi { contract: ell, token0: t0, token1: t1 }
        })])
        .collect();
    let wants: Vec<Want> = refs
        .iter()
        .map(|(fam, r)| {
            let (token0, token1) = match r {
                PoolRef::Multi { token0, token1, .. } => (*token0, *token1),
                _ => pair("ellipsis_2026-11-15", "USDC", "USDT"),
            };
            Want { family: fam, reference: r, spacing: None, step: None, usdt: token1, token0, token1 }
        })
        .collect();
    let groups = load_site::groups(&wants);
    assert_eq!(groups, vec![(pool, vec![0, 1, 2]), (ell, vec![4])], "три пары Nerve — одно чтение; Contract — мимо");
}

#[test]
fn one_read_gives_every_pair() {
    // Nerve: один Raw → три пары = прежний разбор каждой пары.
    let probe = rec("nerve_2026-11-15", "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let (calls, tail) = load_saddle::calls(pool);
    let (head, t) = loader_answers(&probe, &calls, tail, ["nerve getToken", "nerve getTokenBalance"], &["nerve getAPrecise()", "nerve swapStorage()", "nerve paused()"]);
    let dec = decimals(&probe, "nerve", "getToken", 3);
    let n = head.len();
    let raw = load_saddle::Raw { pool, got: head.iter().cloned().chain(t.iter().cloned()).collect(), tail: n, decimals: dec.clone() };
    let mut rest = Vec::new();
    for (a, b) in PAIRS3 {
        let pr = pair("nerve_2026-11-15", a, b);
        let one = load_saddle::state_with(&raw, pr, &bad).unwrap();
        let old = load_saddle::build(load_saddle::parse(&head, &t, pr, &bad).unwrap(), &dec, &bad).unwrap();
        assert_eq!(one, old, "nerve {a}/{b}");
        rest.push(split(&one));
    }
    assert!(rest.windows(2).all(|w| w[0].1 == w[1].1), "у пар одно состояние пула");
    assert_eq!(rest.iter().map(|r| r.0).collect::<std::collections::HashSet<_>>().len(), 3, "индексы — свои у пары");
    // Ellipsis: то же.
    let probe = rec("ellipsis_2026-11-15", "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let (calls2, tail) = load_curve::calls(pool);
    let (head, t) = loader_answers(&probe, &calls2, tail, ["ellipsis coins", "ellipsis balances"], &["ellipsis A()", "ellipsis fee()"]);
    let dec = decimals(&probe, "ellipsis", "coins", 3);
    let n = head.len();
    let raw = load_curve::Raw { pool, got: head.iter().cloned().chain(t.iter().cloned()).collect(), tail: n, decimals: dec.clone() };
    for (a, b) in PAIRS3 {
        let pr = pair("ellipsis_2026-11-15", a, b);
        let (_, bal, amp, fee, i0, i1) = load_curve::parse(&head, &t, pr, &bad).unwrap();
        let old = load_curve::build(bal, amp, fee, (i0, i1), &dec, &bad).unwrap();
        assert_eq!(load_curve::state_with(&raw, pr, &bad).unwrap(), old, "ellipsis {a}/{b}");
    }
}

#[test]
fn late_pairs_follow_their_contract() {
    // Ревью 295, п. 2: поздние пулы с парами Multi — сначала пары контракта открытой пачки, затем группами по контракту.
    let nerve = Address::from_str(rec("nerve_2026-11-15", "probe.json")["pool"].as_str().unwrap()).unwrap();
    let ell = Address::from_str(rec("ellipsis_2026-11-15", "probe.json")["pool"].as_str().unwrap()).unwrap();
    let f = families::make("nerve", "saddle_stableswap").unwrap();
    let found = |contract: Address, dir: &str, a: &str, b: &str| {
        let (t0, t1) = pair(dir, a, b);
        let r = PoolRef::Multi { contract, token0: t0, token1: t1 };
        (arb::venues::bsc_dex::pools::verify_woo::found(&arb::venues::bsc_dex::pools::verify::Candidate { family: f, pool: format!("{contract:#x}"), coin: t0, quote: t1 }, r), String::new())
    };
    let list = vec![
        found(ell, "ellipsis_2026-11-15", "BUSD", "USDT"),
        found(nerve, "nerve_2026-11-15", "BUSD", "USDT"),
        found(ell, "ellipsis_2026-11-15", "USDC", "USDT"),
        found(nerve, "nerve_2026-11-15", "USDC", "USDT"),
    ];
    let open = std::collections::HashSet::from([PoolRef::Contract(nerve)]);
    let got: Vec<Address> = arb::venues::bsc_dex::pools::attach::by_site(list, &open).iter().map(|(f, _)| f.reference.address()).collect();
    assert_eq!(got, vec![nerve, nerve, ell, ell], "пары контракта открытой пачки — первыми, затем подряд по контракту");
}
