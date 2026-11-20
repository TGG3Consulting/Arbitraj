//! Файл: tests/arbitrum_balancer_v3.rs
//! Что проверяет: решение 441 (T433-F2) — Balancer V3 на Arbitrum на РЕАЛЬНЫХ ответах своего узла, записанных на одном
//! блоке скриптом tools/arbitrum_record/balancer_v3.py (папки tests/data/arbitrum_balancer_v3_<дата>/):
//! 1. Выпуск пула по записанным isPoolFromFactory (balancer_v3_factories::release) и крючки (swap_hook) — пулы с крючком
//!    обмена и пулы не из фабрик только считаются (их котировка не сверяется: бот их не берёт).
//! 2. Состояние — тем же разбором, что у чтения на узле (load_balancer_v3::decode по ответам в порядке calls).
//! 3. Расчёт бота (math/balancer_math.rs → math/balancer_v3.rs) на записанном состоянии = ответу Router V3
//!    querySwapSingleTokenExactIn того же блока для каждой пары и суммы; откат роутера — расчёт None.
//!
//! Пока записи нет — тесты помечены #[ignore] (запуск после записи: cargo test --test arbitrum_balancer_v3 -- --ignored;
//! затем метку снять).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{abi_balancer_v3, balancer_v3_factories, load_balancer_v3,
//! load_balancer_v3_fixed}, arb::venues::bsc_dex::math::balancer_math, serde_json. Моков нет.

use std::path::{Path, PathBuf};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::math::balancer_math;
use arb::venues::bsc_dex::pools::abi_balancer_v3::{
    IBalancerV3Factory as F, IBalancerV3Pool as P, IBalancerV3Vault as V,
};
use arb::venues::bsc_dex::pools::balancer_v3_factories::{self as fac, Release};
use arb::venues::bsc_dex::pools::load_balancer_v3;
use arb::venues::bsc_dex::pools::load_balancer_v3_fixed::{Fixed, swap_hook};
use arb::venues::bsc_dex::quote::quoter_balancer_v3;

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

/// Файлы пулов папки записи.
fn pools(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        if name.starts_with("pool_") && name.ends_with(".json") {
            out.push(serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap());
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

/// Выпуск по имени из meta.json.
fn release(name: &str) -> Release {
    match name {
        "weighted" => Release::Weighted,
        "weighted_min" => Release::WeightedMin,
        "stable" => Release::Stable,
        "stable_guard" => Release::StableGuard,
        x => panic!("выпуск {x}"),
    }
}

/// Неизменяемое пула по записи (то же, что load_balancer_v3_fixed::fixed). None — пул бот не берёт (крючок обмена,
/// не из фабрик, откат хранилища).
fn fixed(p: &Value) -> Option<Fixed> {
    let f = &p["fixed"];
    let b = answer(&f["getHooksConfig"]).ok()?;
    let hooks = V::getHooksConfigCall::abi_decode_returns(&b).ok()?;
    if swap_hook(&hooks).is_some() {
        return None;
    }
    let b = answer(&f["getPoolTokenInfo"]).ok()?;
    let info = V::getPoolTokenInfoCall::abi_decode_returns(&b).ok()?;
    let min_trade = answer(&f["getMinimumTradeAmount"]).unwrap();
    let min_trade = V::getMinimumTradeAmountCall::abi_decode_returns(&min_trade).unwrap();
    let mut list = Vec::new();
    let mut yes = Vec::new();
    for x in f["isPoolFromFactory"].as_array().unwrap() {
        let a: Address = x["factory"].as_str().unwrap().parse().unwrap();
        list.push((a, release(x["release"].as_str().unwrap())));
        let ok = answer(x)
            .ok()
            .and_then(|b| F::isPoolFromFactoryCall::abi_decode_returns(&b).ok())
            .unwrap_or(false);
        yes.push(ok);
    }
    let r = fac::release(&list, &yes).ok()?;
    let weights = if r.weighted() {
        let b = answer(&f["getNormalizedWeights"]).unwrap();
        P::getNormalizedWeightsCall::abi_decode_returns(&b).unwrap()
    } else {
        Vec::new()
    };
    let min_balances = if r == Release::WeightedMin {
        let b = answer(&f["getMinTokenBalances"]).unwrap();
        P::getMinTokenBalancesCall::abi_decode_returns(&b).unwrap()
    } else {
        Vec::new()
    };
    Some(Fixed {
        release: r,
        tokens: info.tokens,
        weights,
        min_balances,
        min_trade,
    })
}

/// Ответы пачки чтения в порядке load_balancer_v3::calls.
fn got(p: &Value, fx: &Fixed) -> Vec<Result<Bytes, String>> {
    let s = &p["state"];
    let mut names = vec![
        "getPoolTokenInfo",
        "getCurrentLiveBalances",
        "getPoolTokenRates",
        "getPoolConfig",
        "isPoolPaused",
        "isVaultPaused",
    ];
    if !fx.release.weighted() {
        names.push("getAmplificationParameter");
    }
    names.iter().map(|n| answer(&s[*n])).collect()
}

/// Расчёт бота = ответ Router V3 на каждой записанной сумме (откат роутера — None у бота).
#[test]
#[ignore = "нет записи: tools/arbitrum_record/balancer_v3.py"]
fn quote_matches_router_v3() {
    let (mut same, mut skipped) = (0usize, 0usize);
    for dir in dirs() {
        for p in pools(&dir) {
            let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
            let Some(fx) = fixed(&p) else {
                skipped += 1;
                continue;
            };
            let answers = got(&p, &fx);
            for q in p["quotes"].as_array().unwrap() {
                let i = q["i"].as_u64().unwrap() as usize;
                let j = q["j"].as_u64().unwrap() as usize;
                let pair = (fx.tokens[i], fx.tokens[j]);
                let st = load_balancer_v3::decode(pool, &fx, &answers, pair).unwrap();
                let PoolState::VaultPool(d) = st else {
                    panic!("не VaultPool");
                };
                let amount = U256::from_str_radix(q["amount"].as_str().unwrap(), 10).unwrap();
                let ours = balancer_math::amount_out(&d, i, j, amount);
                let theirs = answer(q).ok().and_then(|b| quoter_balancer_v3::decode(&b));
                assert_eq!(ours, theirs, "пул {pool:#x} {i}→{j} сумма {amount}");
                same += 1;
            }
        }
    }
    assert!(same > 0, "ни одной сверки (пропущено пулов: {skipped})");
}
