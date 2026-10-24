//! Файл: tests/bsc_dex_node_calls.rs
//! Что делает: собирает список ТОЧНЫХ вызовов узла BSC для записи живых ответов (запрос техлида по проверке #45):
//! проверка пулов CAKE и ETH из записи GeckoTerminal 2026-10-24 — поля пулов V2/V3 (factory, token0, token1, fee,
//! tickSpacing, lmPool), реестр фабрик V2 (getPair), ключи Infinity CL (poolIdToPoolKey) и Uniswap V4 (PositionManager
//! poolKeys, poolManager), и всё вместе одним Multicall3.aggregate3. Пишет tests/data/node_calls_2026-10-24.json.
//! Запуск (вручную, не в CI): cargo test --test bsc_dex_node_calls -- --ignored
//! Кто вызывает: человек.
//! От чего зависит: arb::venues::bsc_dex::{directory::gt_parse, pools::abi_*, node::multicall}.
//! Настройки: нет. Адреса — официальные страницы адресов (indexer_research.md, 2.1; Uniswap v4 deployments, BNB).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, FixedBytes, address};
use serde_json::{Value, json};

use arb::venues::bsc_dex::directory::gt_parse;
use arb::venues::bsc_dex::node::multicall;
use arb::venues::bsc_dex::pools::abi_check::{
    IInfinityKeys, IPoolImmutables as P, IPositionManagerKeys,
};
use arb::venues::bsc_dex::pools::abi_v2v3::IV2Factory;

const MULTICALL3: Address = address!("0xcA11bde05977b3631167028862bE2a173976CA11");
const PCS_V2_FACTORY: Address = address!("0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73");
const CL_MANAGER: Address = address!("0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b");
const UNI_POSM: Address = address!("0x7a4a5c919ae2541aed11041a1aeee68f1287f95b");

/// Один eth_call для записи.
fn call(name: String, to: Address, data: Bytes) -> Value {
    json!({ "name": name, "method": "eth_call", "params": [{ "to": to, "input": data }, "latest"] })
}

#[test]
#[ignore = "вручную: пишет список вызовов для записи ответов узла"]
fn write_node_calls() {
    let text =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .unwrap();
    let rec: Value = serde_json::from_str(&text).unwrap();
    let mut out = vec![json!({ "name": "block", "method": "eth_blockNumber", "params": [] })];
    let mut all: Vec<(Address, Bytes)> = Vec::new();
    let mut push = |out: &mut Vec<Value>, name: String, to: Address, data: Bytes| {
        out.push(call(name, to, data.clone()));
        all.push((to, data));
    };
    for coin in ["CAKE", "ETH"] {
        let pools =
            gt_parse::pools(rec[format!("gt_pools_{coin}_p1")]["body"].as_str().unwrap()).unwrap();
        for p in pools {
            let tag = format!("{coin} {} {}", p.dex_id, p.pool);
            if p.pool.len() == 42 {
                let a: Address = p.pool.parse().unwrap();
                let fields = [
                    ("factory()", P::factoryCall {}.abi_encode()),
                    ("token0()", P::token0Call {}.abi_encode()),
                    ("token1()", P::token1Call {}.abi_encode()),
                    ("fee()", P::feeCall {}.abi_encode()),
                    ("tickSpacing()", P::tickSpacingCall {}.abi_encode()),
                    ("lmPool()", P::lmPoolCall {}.abi_encode()),
                ];
                for (n, d) in fields {
                    push(&mut out, format!("{tag} {n}"), a, Bytes::from(d));
                }
                if p.dex_id == "pancakeswap_v2" {
                    let (t0, t1) = (p.base.min(p.quote), p.base.max(p.quote));
                    let d = IV2Factory::getPairCall {
                        tokenA: t0,
                        tokenB: t1,
                    }
                    .abi_encode();
                    push(
                        &mut out,
                        format!("{tag} PancakeSwap V2 getPair"),
                        PCS_V2_FACTORY,
                        Bytes::from(d),
                    );
                }
            } else {
                let id: B256 = p.pool.parse().unwrap();
                if p.dex_id == "pancakeswap-infinity-clmm" {
                    let d = IInfinityKeys::poolIdToPoolKeyCall { id }.abi_encode();
                    push(
                        &mut out,
                        format!("{tag} CLPoolManager poolIdToPoolKey"),
                        CL_MANAGER,
                        Bytes::from(d),
                    );
                } else if p.dex_id == "uniswap-v4-bsc" {
                    let short = FixedBytes::<25>::from_slice(&id[..25]);
                    let d = IPositionManagerKeys::poolKeysCall { poolId: short }.abi_encode();
                    push(
                        &mut out,
                        format!("{tag} PositionManager poolKeys"),
                        UNI_POSM,
                        Bytes::from(d),
                    );
                }
            }
        }
    }
    let pm = IPositionManagerKeys::poolManagerCall {}.abi_encode();
    push(
        &mut out,
        "Uniswap V4 PositionManager poolManager()".into(),
        UNI_POSM,
        Bytes::from(pm),
    );
    let n = all.len();
    out.push(call(
        format!("Multicall3 aggregate3 всех {n} вызовов выше"),
        MULTICALL3,
        multicall::encode(&all),
    ));
    let doc = json!({
        "about": "Точные вызовы узла BSC для записи (tests/bsc_dex_node_calls.rs). Сначала eth_blockNumber, затем все eth_call на том же блоке: в params вместо \"latest\" поставить номер блока из первого ответа (0x…). Ответы — как вернул узел: {name, request, response}. V3 getPool — вторым проходом по ответам fee().",
        "calls": out,
    });
    std::fs::write(
        "tests/data/node_calls_2026-10-24.json",
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap();
}
