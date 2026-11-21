//! Файл: tests/bsc_dex_multi_pair_verify.rs
//! Что делает: ревью решения 295, п. 4 — связка «разбор проверки на узле → multi_pair::adopt» настоящими функциями
//! verify_* (judge — то, что вызывает verify после ответов узла) на ЖИВЫХ записанных ответах:
//! - Ellipsis 3pool: verify_curve::judge на coins(0..7) (tests/data/stable_list_2026-11-15/token_calls.json);
//! - Nerve 3pool: verify_stable_u::judge на getToken(0..31) (та же запись) и dex.stable_pools;
//! - Balancer ACSI (пулы на 4 и 5 токенов): verify_balancer::judge на ответах второго прохода — ответы ПЕРЕКОДИРОВАНЫ
//!   abi_encode из записанных значений probe.json (getPool, getPoolTokens, isPoolFromFactory), сырых байтов узла в записи
//!   нет (как tests/bsc_dex_balancer_acsi_live.rs second);
//! - Curve NG (трёхмонетный пул): verify_curve_ng::judge на get_coins / is_meta / version (probe.json).
//!
//! При dex.multi_pair = да — ссылка Multi с парой кандидата и монеты пула в справочнике (pool_tokens); без настройки —
//! Contract и монеты не запоминаются. PancakeSwap StableSwap — записанных ответов трёхмонетного пула нет (TASKS 295).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_verify.
//! От чего зависит: tests/multi_common, arb::venues::bsc_dex::pools::{verify_*, verify::Candidate}.

mod multi_common;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::verify::{Candidate, Verdict};
use arb::venues::bsc_dex::pools::{
    families, verify_balancer, verify_curve, verify_curve_ng, verify_stable_u,
};

use multi_common::{ctx, rec, set_dex};

/// Ответы записи token_calls.json по префиксу имени (Ok — результат, Err — откат). Паника — нет записи.
fn token_calls(pre: &str) -> (Address, Vec<Result<Bytes, String>>) {
    let r = rec("stable_list_2026-11-15", "token_calls.json");
    let calls: Vec<&Value> = r["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"].as_str().unwrap().starts_with(pre))
        .collect();
    let to = calls[0]["request"]["params"][0]["to"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let a = calls
        .iter()
        .map(|c| match c["response"]["result"].as_str() {
            Some(h) => Ok(Bytes::from(alloy_primitives::hex::decode(h).unwrap())),
            None => Err(c["response"]["error"]["message"].to_string()),
        })
        .collect();
    (to, a)
}

/// Адрес из строки или слова ответа. Паника — не адрес.
fn addr(v: &Value) -> Address {
    let s = v.as_str().unwrap();
    if s.len() == 66 {
        return Address::from_slice(&alloy_primitives::hex::decode(&s[2..]).unwrap()[12..32]);
    }
    s.parse().unwrap()
}

/// Окружение DEX `id` вида `kind`, режим `on`. Ошибок нет (паника теста).
async fn env(id: &str, kind: &str, on: bool, pool: Address) -> Ctx {
    let c = ctx(&format!("verify-{id}-{on}-{pool}")).await;
    set_dex(&c, id, "dex.kind", json!(kind)).await;
    set_dex(&c, id, "dex.stable_pools", json!([format!("{pool:#x}")])).await;
    if on {
        set_dex(&c, id, "dex.multi_pair", json!(true)).await;
    }
    c
}

/// Проверка итога: в режиме — Multi пары кандидата и монеты пула в справочнике; без — Contract.
fn expect(
    c: &Ctx,
    v: Result<arb::venues::bsc_dex::pools::discover::Found, String>,
    pool: Address,
    tokens: &[Address],
    on: bool,
) {
    let f = v.expect("пул принят");
    let k = format!("{pool:#x}");
    if on {
        assert!(
            matches!(f.reference, PoolRef::Multi { contract, token0, token1 } if contract == pool && (token0, token1) == (f.token0, f.token1))
        );
        assert_eq!(
            c.index.with(|s| s.pool_tokens.get(&k).cloned()),
            Some(tokens.to_vec())
        );
    } else {
        assert_eq!(f.reference, PoolRef::Contract(pool));
        assert!(c.index.with(|s| s.pool_tokens.is_empty()));
    }
}

/// Итог Verdict → Result. Ошибок нет.
fn taken(v: Verdict) -> Result<arb::venues::bsc_dex::pools::discover::Found, String> {
    match v {
        Verdict::Taken(f) => Ok(*f),
        Verdict::Rejected(w) | Verdict::Failed(w) => Err(w),
    }
}

#[tokio::test]
async fn nerve_and_ellipsis_judge_then_adopt() {
    for on in [true, false] {
        let (pool, a) = token_calls("ellipsis coins(");
        let tokens = arb::venues::bsc_dex::pools::load_curve::coins_of(&a).unwrap();
        let c = env("ellipsis", "curve_stableswap", on, pool).await;
        let fam = families::make("ellipsis", "curve_stableswap").unwrap();
        let cand = Candidate {
            family: fam,
            pool: format!("{pool:#x}"),
            coin: tokens[0],
            quote: tokens[2],
        };
        expect(
            &c,
            taken(verify_curve::judge(&c, &cand, pool, &a)),
            pool,
            &tokens,
            on,
        );
        let (pool, a) = token_calls("nerve getToken(");
        let tokens = arb::venues::bsc_dex::pools::load_saddle::tokens_of(&a).unwrap();
        let c = env("nerve", "saddle_stableswap", on, pool).await;
        let fam = families::make("nerve", "saddle_stableswap").unwrap();
        let cand = Candidate {
            family: fam,
            pool: format!("{pool:#x}"),
            coin: tokens[1],
            quote: tokens[2],
        };
        expect(
            &c,
            verify_stable_u::judge(&c, &cand, pool, &a, a.len()),
            pool,
            &tokens,
            on,
        );
    }
}

#[tokio::test]
async fn balancer_and_curve_ng_judge_then_adopt() {
    let probe = rec("balancer_acsi_2026-11-15", "probe.json");
    let dec = |v: &Value| U256::from_str_radix(v.as_str().unwrap(), 10).unwrap();
    let fam = families::make("acsi-finance", "balancer_v2").unwrap();
    for on in [true, false] {
        for a in [
            "0x894ed9026de37afd9cce1e6c0be7d6b510e3ffe5",
            "0xe901c4e9a56a175cb5c94bfb5f821ebd5fc826c5",
        ] {
            let p = probe["pools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["pool"] == a)
                .unwrap();
            let pool: Address = a.parse().unwrap();
            let t: Vec<Address> = p["tokens"].as_array().unwrap().iter().map(addr).collect();
            let bal: Vec<U256> = p["balances"].as_array().unwrap().iter().map(dec).collect();
            let ret = |b: bool| Ok(Bytes::from(b.abi_encode()));
            let answers = vec![
                Ok(Bytes::from(
                    (addr(&p["getPool"][0]), dec(&p["getPool"][1])).abi_encode_params(),
                )),
                Ok(Bytes::from(
                    (t.clone(), bal, dec(&p["lastChangeBlock"])).abi_encode_params(),
                )),
                ret(p["isPoolFromFactory"]["weighted"] == "1"),
                ret(p["isPoolFromFactory"]["stable"] == "1"),
            ];
            let plan = (
                pool,
                addr(&probe["vault"]["address"]),
                addr(&probe["factories"]["weighted"]["address"]),
                Some(addr(&probe["factories"]["stable"]["address"])),
            );
            let c = env("acsi-finance", "balancer_v2", on, pool).await;
            let cand = Candidate {
                family: fam,
                pool: a.into(),
                coin: t[1],
                quote: t[0],
            };
            expect(
                &c,
                taken(verify_balancer::judge(&c, &cand, plan, &answers)),
                pool,
                &t,
                on,
            );
        }
        let ng = rec("curve_ng_2026-11-14", "probe.json");
        let p = ng["pools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["coins"].as_array().unwrap().len() == 3)
            .unwrap();
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        let t: Vec<Address> = p["coins"].as_array().unwrap().iter().map(addr).collect();
        let h = |v: &Value| {
            Ok(Bytes::from(
                alloy_primitives::hex::decode(&v.as_str().unwrap()[2..]).unwrap(),
            ))
        };
        let answers = vec![
            h(&p["factory"]["get_coins"]),
            h(&p["factory"]["is_meta"]),
            h(&p["calls"]["version"]),
        ];
        let c = env("curve-bsc", "curve_stableswap_ng", on, pool).await;
        let fam = families::make("curve-bsc", "curve_stableswap_ng").unwrap();
        let cand = Candidate {
            family: fam,
            pool: format!("{pool:#x}"),
            coin: t[2],
            quote: t[0],
        };
        let v = verify_curve_ng::judge(&c, &cand, pool, &["v7.0.0".to_string()], &answers);
        expect(&c, taken(v), pool, &t, on);
    }
}
