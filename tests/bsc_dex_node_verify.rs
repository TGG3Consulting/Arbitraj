//! Файл: tests/bsc_dex_node_verify.rs
//! Что делает: проверка пулов из справочника на ЖИВЫХ ответах узла BSC (решения 58, 59): публичный узел
//! bsc-dataseed.bnbchain.org, блок 0x7b8a659, 2026-10-24 (tests/data/indexers_2026-10-24/arb_node_responses_2026-10-24.json,
//! вызовы — node_calls_2026-10-24.json). Пулы CAKE и ETH из записи GeckoTerminal: поля пулов (factory, token0, token1,
//! fee, tickSpacing, lmPool), реестр PancakeSwap V2 (getPair), ключи PancakeSwap Infinity (poolIdToPoolKey) и Uniswap V4
//! (PositionManager poolKeys, poolManager), ответ Multicall3.aggregate3 тех же вызовов.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{directory::gt_parse, node::multicall, pools::{families, verify, verify_v4}}.
//! Настройки: нет. Адреса фабрик и менеджеров — официальные страницы адресов (indexer_research.md, 2.1).

use std::collections::HashMap;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Bytes, address};
use serde_json::Value;

use arb::venues::bsc_dex::directory::gt_parse::{self, IndexedPool};
use arb::venues::bsc_dex::node::multicall;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::verify::{self, Candidate, Read};
use arb::venues::bsc_dex::pools::verify_v4;

const DIR: &str = "tests/data/indexers_2026-10-24/";
const PCS_V2: Address = address!("0xca143ce32fe78f1f7019d7d551a6402fc5350c73");
const PCS_V3: Address = address!("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const UNI_V3: Address = address!("0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7");
const CL_MANAGER: Address = address!("0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b");
const UNI_POOL_MANAGER: Address = address!("0x28e2ea090877bf75740558f6bfb36a5ffee9e9df");
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
const CAKE: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");

/// Записанные ответы eth_call: имя вызова → байты ответа или текст отката узла.
fn answers() -> HashMap<String, Result<Bytes, String>> {
    let text = std::fs::read_to_string(format!("{DIR}arb_node_responses_2026-10-24.json")).unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["request"]["method"] == "eth_call")
        .map(|r| {
            let resp = &r["response"];
            let v = match resp["result"].as_str() {
                Some(h) => Ok(Bytes::from_str(h).unwrap()),
                None => Err(resp["error"]["message"].as_str().unwrap().to_string()),
            };
            (r["name"].as_str().unwrap().to_string(), v)
        })
        .collect()
}

/// Пулы монеты из записи GeckoTerminal.
fn pools(coin: &str) -> Vec<IndexedPool> {
    let text = std::fs::read_to_string(format!("{DIR}arb_indexers_2026-10-24.json")).unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    gt_parse::pools(rec[format!("gt_pools_{coin}_p1")]["body"].as_str().unwrap()).unwrap()
}

/// Ответы полей пула в порядке проверки (factory, token0, token1[, fee, tickSpacing, lmPool]).
fn fields(
    a: &HashMap<String, Result<Bytes, String>>,
    coin: &str,
    p: &IndexedPool,
    v3: bool,
) -> Vec<Result<Bytes, String>> {
    let mut names = vec!["factory()", "token0()", "token1()"];
    if v3 {
        names.extend(["fee()", "tickSpacing()", "lmPool()"]);
    }
    names
        .iter()
        .map(|n| a[&format!("{coin} {} {} {n}", p.dex_id, p.pool)].clone())
        .collect()
}

/// Вид и фабрика DEX индексатора, как их задаст Тигран (официальные виды, решение 59).
fn row(dex: &str) -> Option<(&'static str, Address)> {
    match dex {
        "pancakeswap_v2" => Some(("pancakeswap_v2", PCS_V2)),
        "pancakeswap-v3-bsc" => Some(("pancakeswap_v3", PCS_V3)),
        "uniswap-bsc" => Some(("uniswap_v3", UNI_V3)),
        _ => None,
    }
}

/// Проверка полей одного пула видом `kind` с фабрикой `factory`. Возвращает: прочитанное или причину отказа.
fn check(
    a: &HashMap<String, Result<Bytes, String>>,
    coin: &str,
    p: &IndexedPool,
    kind: &str,
    factory: Address,
) -> Result<Read, String> {
    let f = families::make(p.dex_id.as_str(), kind).unwrap();
    let v3 = kind != "pancakeswap_v2";
    let mut r = Read::default();
    verify::fill(f, &mut r, &fields(a, coin, p, v3))?;
    let c = Candidate {
        family: f,
        pool: p.pool.clone(),
        coin: p.base,
        quote: p.quote,
    };
    verify::check(&c, factory, &r)?;
    Ok(r)
}

#[test]
fn official_pools_pass_and_v2_registry_matches() {
    let a = answers();
    let mut passed = 0;
    for coin in ["CAKE", "ETH"] {
        for p in pools(coin).iter().filter(|p| p.pool.len() == 42) {
            let Some((kind, factory)) = row(&p.dex_id) else {
                continue;
            };
            let r = check(&a, coin, p, kind, factory)
                .unwrap_or_else(|w| panic!("{coin} {} {}: {w}", p.dex_id, p.pool));
            if kind == "pancakeswap_v2" {
                let reg = a[&format!("{coin} {} {} PancakeSwap V2 getPair", p.dex_id, p.pool)]
                    .clone()
                    .unwrap();
                verify::registered(p.pool.parse().unwrap(), &reg).unwrap();
            } else {
                assert!(r.fee.is_some() && r.spacing.is_some());
            }
            passed += 1;
        }
    }
    // PancakeSwap V2 — 12, PancakeSwap V3 — 11, Uniswap V3 — 6 пулов в записи CAKE и ETH: все прошли.
    assert_eq!(passed, 29);
}

#[test]
fn foreign_factories_rejected() {
    let a = answers();
    let find = |coin: &str, dex: &str| pools(coin).into_iter().find(|p| p.dex_id == dex).unwrap();
    // PancakeSwap V1 (CAKE/WBNB) под видом pancakeswap_v2: своя фабрика 0xbcfc… — отказ (Universal Router V2 менял бы в паре V2).
    let v1 = find("CAKE", "pancakeswap-v1-bsc");
    let why = check(&a, "CAKE", &v1, "pancakeswap_v2", PCS_V2).unwrap_err();
    assert!(
        why.starts_with("factory() пула = 0xBCfCcbde45cE874adCB698cC183deBcF17952812"),
        "{why}"
    );
    // THENA V3 и Topaz отвечают как Uniswap V3 (fee, tickSpacing, нет lmPool), но фабрики у них свои — отказ.
    for dex in ["thena-v3", "topaz"] {
        let p = find("ETH", dex);
        let why = check(&a, "ETH", &p, "uniswap_v3", UNI_V3).unwrap_err();
        assert!(why.starts_with("factory() пула ="), "{dex}: {why}");
    }
}

#[test]
fn lm_pool_tells_pancake_from_uniswap() {
    let a = answers();
    for coin in ["CAKE", "ETH"] {
        for p in pools(coin)
            .iter()
            .filter(|p| p.dex_id == "pancakeswap-v3-bsc" || p.dex_id == "uniswap-bsc")
        {
            let got = fields(&a, coin, p, true);
            let lm = got.get(5);
            let pcs = families::make("x-pcs", "pancakeswap_v3").unwrap();
            let uni = families::make("x-uni", "uniswap_v3").unwrap();
            if p.dex_id == "pancakeswap-v3-bsc" {
                // У PancakeSwap V3 lmPool() отвечает всегда (бывает и нулевой адрес — пул 0xeb75…), у Uniswap — откат.
                assert!(
                    verify::v3_abi(pcs, lm).is_ok() && verify::v3_abi(uni, lm).is_err(),
                    "{}",
                    p.pool
                );
            } else {
                assert!(
                    verify::v3_abi(uni, lm).is_ok() && verify::v3_abi(pcs, lm).is_err(),
                    "{}",
                    p.pool
                );
            }
        }
    }
}

#[test]
fn infinity_keys_from_manager() {
    let a = answers();
    let f = families::make("pancakeswap-infinity-clmm", "pancakeswap_infinity_cl").unwrap();
    let key = |pool: &str| {
        a[&format!("CAKE pancakeswap-infinity-clmm {pool} CLPoolManager poolIdToPoolKey")].clone()
    };
    for p in pools("CAKE")
        .iter()
        .filter(|p| p.dex_id == "pancakeswap-infinity-clmm")
    {
        let id = B256::from_str(&p.pool).unwrap();
        let c = Candidate {
            family: f,
            pool: p.pool.clone(),
            coin: CAKE,
            quote: if p.quote == CAKE { p.base } else { p.quote },
        };
        let r = verify_v4::decide(&c, id, CL_MANAGER, key(&p.pool), Ok(Bytes::new()));
        if c.quote == USDT {
            let found = r.unwrap_or_else(|w| panic!("{}: {w}", p.pool));
            assert_eq!(found.coin, CAKE);
            assert!(found.tick_spacing.is_some());
        } else {
            // Пулы с нативным BNB: ключ у менеджера есть, валюта 0x0 — проходят как пул «CAKE — 0x0», не USDT.
            assert_eq!(c.quote, Address::ZERO);
            assert!(r.is_ok(), "{}", p.pool);
            let with_usdt = Candidate { quote: USDT, ..c };
            assert!(
                verify_v4::decide(&with_usdt, id, CL_MANAGER, key(&p.pool), Ok(Bytes::new()))
                    .unwrap_err()
                    .starts_with("валюты пула")
            );
        }
        // Другой менеджер в настройках — отказ.
        let other = verify_v4::decide(
            &c_with(f, p),
            id,
            UNI_POOL_MANAGER,
            key(&p.pool),
            Ok(Bytes::new()),
        );
        assert!(
            other
                .unwrap_err()
                .starts_with("менеджер не знает этот PoolId")
        );
    }
}

/// Кандидат пула Infinity CAKE с его встречной валютой.
fn c_with(f: &'static families::Family, p: &IndexedPool) -> Candidate {
    let quote = if p.quote == CAKE { p.base } else { p.quote };
    Candidate {
        family: f,
        pool: p.pool.clone(),
        coin: CAKE,
        quote,
    }
}

#[test]
fn uniswap_v4_keys_from_position_manager() {
    let a = answers();
    let f = families::make("uniswap-v4-bsc", "uniswap_v4").unwrap();
    let pm = a["Uniswap V4 PositionManager poolManager()"].clone();
    for coin in ["CAKE", "ETH"] {
        let coin_addr = pools(coin)[0].base;
        for p in pools(coin).iter().filter(|p| p.dex_id == "uniswap-v4-bsc") {
            let id = B256::from_str(&p.pool).unwrap();
            let key =
                a[&format!("{coin} uniswap-v4-bsc {} PositionManager poolKeys", p.pool)].clone();
            let other = if p.base == coin_addr { p.quote } else { p.base };
            // Пары из записи — мусорные токены («狗狗币 / Cake», «B13B / ETH»): ключ у PositionManager есть и даёт тот же
            // PoolId, PositionManager работает с PoolManager из настроек — пул настоящий, но не с USDT.
            let c = Candidate {
                family: f,
                pool: p.pool.clone(),
                coin: coin_addr,
                quote: other,
            };
            let found = verify_v4::decide(&c, id, UNI_POOL_MANAGER, key.clone(), pm.clone())
                .unwrap_or_else(|w| panic!("{}: {w}", p.pool));
            assert_eq!(found.coin, coin_addr);
            let usdt = Candidate { quote: USDT, ..c };
            assert!(
                verify_v4::decide(&usdt, id, UNI_POOL_MANAGER, key, pm.clone())
                    .unwrap_err()
                    .starts_with("валюты пула")
            );
        }
    }
    // Менеджер в настройках не тот, с которым работает PositionManager — отказ.
    let p = pools("ETH")
        .into_iter()
        .find(|p| p.dex_id == "uniswap-v4-bsc")
        .unwrap();
    let key = a[&format!("ETH uniswap-v4-bsc {} PositionManager poolKeys", p.pool)].clone();
    let c = Candidate {
        family: f,
        pool: p.pool.clone(),
        coin: p.base,
        quote: p.quote,
    };
    let why =
        verify_v4::decide(&c, B256::from_str(&p.pool).unwrap(), CL_MANAGER, key, pm).unwrap_err();
    assert!(
        why.starts_with("PositionManager работает с менеджером"),
        "{why}"
    );
}

#[test]
fn multicall_matches_single_calls() {
    let text = std::fs::read_to_string(format!("{DIR}arb_node_responses_2026-10-24.json")).unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    let list = rec["results"].as_array().unwrap();
    let single = &list[1..list.len() - 1];
    let mc = Bytes::from_str(list.last().unwrap()["response"]["result"].as_str().unwrap()).unwrap();
    let got = multicall::decode(&mc, single.len()).unwrap();
    for (s, m) in single.iter().zip(got) {
        match s["response"]["result"].as_str() {
            Some(h) => assert_eq!(m.unwrap(), Bytes::from_str(h).unwrap(), "{}", s["name"]),
            None => assert!(
                m.is_err(),
                "{}: отдельно — откат, в aggregate3 — успех",
                s["name"]
            ),
        }
    }
}
