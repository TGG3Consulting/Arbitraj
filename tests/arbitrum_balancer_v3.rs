//! Файл: tests/arbitrum_balancer_v3.rs
//! Что проверяет: решения 441, 452 (T433-F2) — Balancer V3 на Arbitrum на РЕАЛЬНЫХ ответах своего узла, записанных на
//! одном блоке скриптом tools/arbitrum_record/balancer_v3.py (папки tests/data/arbitrum_balancer_v3_<дата>/):
//! 1. Неизменяемое пула — теми же вызовами и разбором, что у бота (load_balancer_v3_fixed::first_calls / first,
//!    load_balancer_v3_extra::calls / decode): выпуск по isPoolFromFactory списков meta.json, крючок (StableSurge /
//!    MEV Capture берутся, прочие крючки обмена — нет). Пулы, которые бот не берёт, только считаются.
//! 2. Состояние — load_balancer_v3::calls / decode (общие вызовы и свои у вида и крючка); ответ берётся из записи по
//!    (адрес, данные) вызова.
//! 3. Расчёт бота (math/balancer_math.rs → math/balancer_v3*.rs, gyro_*.rs, reclamm.rs) на записанном состоянии = ответу
//!    Router V3 того же блока: querySwapSingleTokenExactIn (amount_out) и querySwapSingleTokenExactOut (amount_in, ревью
//!    441 п. 3) для каждой пары и суммы; откат роутера — расчёт None.
//!
//! Пока записи нет — тест помечен #[ignore] (запуск после записи: cargo test --test arbitrum_balancer_v3 -- --ignored;
//! затем метку снять).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{balancer_v3_factories, balancer_v3_hooks, load_balancer_v3,
//! load_balancer_v3_extra, load_balancer_v3_fixed}, arb::venues::bsc_dex::math::balancer_math, serde_json. Моков нет.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::balancer_math;
use arb::venues::bsc_dex::pools::abi_balancer_v3::IBalancerV3Router as R;
use arb::venues::bsc_dex::pools::balancer_v3_factories::{LISTS, Release};
use arb::venues::bsc_dex::pools::balancer_v3_hooks::{HookKind, Lists};
use arb::venues::bsc_dex::pools::load_balancer_v3_fixed::{Fixed, first, first_calls};
use arb::venues::bsc_dex::pools::{load_balancer_v3, load_balancer_v3_extra as extra};

/// Папки записи (tests/data/arbitrum_balancer_v3_*). Нет ни одной — паника с подсказкой.
fn dirs() -> Vec<PathBuf> {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"));
    let mut out: Vec<PathBuf> = std::fs::read_dir(&base)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_balancer_v3_"))
        })
        .collect();
    out.sort();
    assert!(!out.is_empty(), "нет записи: tools/arbitrum_record/balancer_v3.py");
    out
}

/// JSON-файл.
fn json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Файлы пулов папки записи.
fn pools(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        if name.starts_with("pool_") && name.ends_with(".json") {
            out.push(json(&p));
        }
    }
    out
}

/// Ответ записанного вызова: Ok(байты) или Err(текст ошибки узла).
fn answer(v: &Value) -> Result<Bytes, String> {
    match v["result"].as_str() {
        Some(h) => Ok(h.parse().unwrap()),
        None => Err(v["error"].to_string()),
    }
}

/// Адреса списка meta.json["lists"][name].
fn list(meta: &Value, name: &str) -> Vec<Address> {
    meta["lists"][name]
        .as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().parse().unwrap()).collect())
        .unwrap_or_default()
}

/// Ответы на вызовы бота: по (адрес, данные) из записи пула; вызова нет в записи — Err (тест его не подменяет).
fn lookup(
    calls: &HashMap<(Address, Bytes), Result<Bytes, String>>,
    want: &[(Address, Bytes)],
) -> Vec<Result<Bytes, String>> {
    want.iter()
        .map(|k| calls.get(k).cloned().unwrap_or_else(|| Err(format!("нет в записи: {k:?}"))))
        .collect()
}

/// Выход Router V3 на точный вход. None — не по ABI.
fn quoted_in(b: &[u8]) -> Option<U256> {
    R::querySwapSingleTokenExactInCall::abi_decode_returns(b).ok()
}

/// Вход Router V3 на точный выход. None — не по ABI.
fn quoted_out(b: &[u8]) -> Option<U256> {
    R::querySwapSingleTokenExactOutCall::abi_decode_returns(b).ok()
}

/// Расчёт бота = ответ Router V3 на каждой записанной сумме (точный вход и точный выход; откат роутера — None у бота).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/balancer_v3.py"]
fn quote_matches_router_v3() {
    let (mut same, mut skipped) = (0usize, Vec::new());
    for dir in dirs() {
        let meta = json(&dir.join("meta.json"));
        let vault: Address = meta["vault"].as_str().unwrap().parse().unwrap();
        let mut facs: Vec<(Address, Release)> = Vec::new();
        for (name, r) in LISTS {
            facs.extend(list(&meta, name).into_iter().map(|a| (a, r)));
        }
        let hooks = Lists {
            surge: list(&meta, "dex.bv3_stable_surge_hooks"),
            mev: list(&meta, "dex.bv3_mev_capture_hooks"),
        };
        let sys: Address = meta["arb_sys"].as_str().unwrap().parse().unwrap();
        let owner: Address = meta["arb_owner_public"].as_str().unwrap().parse().unwrap();
        for p in pools(&dir) {
            let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
            let mut calls = HashMap::new();
            for c in p["calls"].as_array().unwrap() {
                let to: Address = c["to"].as_str().unwrap().parse().unwrap();
                let data: Bytes = c["data"].as_str().unwrap().parse().unwrap();
                calls.insert((to, data), answer(c));
            }
            let got = lookup(&calls, &first_calls(vault, pool, &facs));
            let (release, tokens, min_trade, hook) = match first(&facs, &hooks, &got) {
                Ok(x) => x,
                Err(w) => {
                    skipped.push(format!("{pool:#x}: {w}"));
                    continue;
                }
            };
            let more = lookup(&calls, &extra::calls(pool, release));
            let fx = Fixed {
                release,
                tokens,
                min_trade,
                hook,
                extra: extra::decode(release, &more).unwrap(),
                buffer: None,
            };
            let pre = matches!(hook, HookKind::Mev(_)).then_some((sys, owner));
            let answers = lookup(&calls, &load_balancer_v3::calls(vault, pool, &fx, pre));
            for (key, exact_in) in [("quotes", true), ("quotes_out", false)] {
                for q in p[key].as_array().into_iter().flatten() {
                    let i = q["i"].as_u64().unwrap() as usize;
                    let j = q["j"].as_u64().unwrap() as usize;
                    let pair = (fx.tokens[i], fx.tokens[j]);
                    let st = match load_balancer_v3::decode(pool, &fx, &answers, pair) {
                        Ok(st) => st,
                        Err(w) => {
                            skipped.push(format!("{pool:#x}: {w}"));
                            break;
                        }
                    };
                    let PoolState::VaultPool(d) = st else {
                        panic!("не VaultPool");
                    };
                    let amount = U256::from_str_radix(q["amount"].as_str().unwrap(), 10).unwrap();
                    let b = answer(q).ok();
                    let (ours, theirs) = if exact_in {
                        let t = b.and_then(|b| quoted_in(&b));
                        (balancer_math::amount_out(&d, i, j, amount), t)
                    } else {
                        let t = b.and_then(|b| quoted_out(&b));
                        (balancer_math::amount_in(&d, i, j, amount), t)
                    };
                    assert_eq!(ours, theirs, "пул {pool:#x} ({release:?}) {i}→{j} {key} {amount}");
                    same += 1;
                }
            }
        }
    }
    assert!(same > 0, "ни одной сверки (не взяты: {skipped:?})");
}
