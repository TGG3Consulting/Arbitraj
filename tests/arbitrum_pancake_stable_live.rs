//! Файл: tests/arbitrum_pancake_stable_live.rs
//! Что делает: решение 471 (T448-10, T433-064) — PancakeSwap StableSwap на Arbitrum One на записи своего узла
//! (tools/arbitrum_record/pancake_stable.py):
//! (1) calldata записи = вызовы бота (alloy-ABI abi_curve / abi_stable_u — те же селекторы);
//! (2) реестр фабрики 0x5D5f…a77a подтверждает пул так же, как verify_stable_u.rs (getPairInfo в обе стороны →
//!   swapContract = пул), у всех пулов реестра (swapPairContract) и пулов API конфигов;
//! (3) код каждого пула кончается теми же метаданными компилятора, что у проверенного исходника (Sourcify 42161
//!   exact_match PancakeStableSwapTwoPool, CBOR ipfs 1220 1e3d34d8…c6b595, solc 0.8.10) — значит, исходник тот же,
//!   что у пулов фабрики BSC (то же CBOR в коде 0x25d0…67e2 и 0x7c76…ae1e) и в официальном байткоде
//!   pancakeswap/infinity-periphery 56a7a4f test/bin/pancakeStableSwapTwoPoolDeployer.bytecode;
//! (4) состояние тем же разбором, что у бота (load_stable_u::head / finish), и get_dy пула = порт get_dy
//!   (`dy = (xp[j] − y − 1)·1e18/rates[j]; dy − fee·dy/1e10` — PancakeStableSwapTwoPool.sol, Sourcify), а при
//!   rates = 1e18 у монеты выхода — и выход `exchange` (exchange_out_t, им считает бот);
//! (5) события TokenExchange из логов пулов разбираются ABI бота (IPancakeStable).
//!
//! Кто вызывает: cargo test -- --ignored (после записи; до записи — #[ignore]).
//! От чего зависит: tests/data/arbitrum_pancake_stable_<дата>/, arb::venues::bsc_dex::{pools::{abi_curve,
//! abi_stable_u, load_stable_u}, math::stableswap_v1}.

use std::path::PathBuf;

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, B256, Bytes, LogData, U256, hex};
use serde_json::Value;

use arb::core::types::PoolState;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::math::stableswap_v1::{
    curve2020, exchange_out_t, fee_denominator, get_y, precision, rate_for_decimals, xp,
};
use arb::venues::bsc_dex::pools::abi_curve::IStableSwapV1;
use arb::venues::bsc_dex::pools::abi_stable_u::{IPancakeStable, IPancakeStableFactory};
use arb::venues::bsc_dex::pools::load_stable_u;

/// Метаданные компилятора в коде пула (Sourcify 42161 0x73ed…6CE0 runtimeBytecode.cborAuxdata).
const CBOR: &str = "a26469706673582212201e3d34d8dc76b32f3f336a45636f73765a13007ed41ad27fa869d83470c6b59564736f6c634300080a0033";
const IGN: &str = "нет записи: tools/arbitrum_record/pancake_stable.py";

/// Последняя по имени папка tests/data/arbitrum_pancake_stable_*. Паника — записи нет.
fn data_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/data")
        .filter_map(|d| d.ok().map(|d| d.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("arbitrum_pancake_stable_"))
        })
        .collect();
    dirs.sort();
    dirs.pop().expect(IGN)
}

fn read(name: &str) -> Value {
    let text = std::fs::read_to_string(data_dir().join(name)).expect(name);
    serde_json::from_str(&text).expect("json")
}

/// Ответ {"ok": "0x…"} → байты; откат — Err(текст).
fn answer(v: &Value) -> Result<Bytes, String> {
    match v.get("ok").and_then(|h| h.as_str()) {
        Some(h) => Ok(Bytes::from(hex::decode(h).expect("hex"))),
        None => Err(v["error"].as_str().unwrap_or("нет ответа").to_string()),
    }
}

fn data(v: &Value) -> Vec<u8> {
    hex::decode(v["data"].as_str().unwrap()).unwrap()
}

fn bad(what: String) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, None, what)
}

fn addr(v: &str) -> Address {
    v.parse().unwrap()
}

/// (1) calldata записи — те же вызовы, что answers() бота: coins(k)/balances(k) k < 8, fee(), A(), is_killed().
#[test]
#[ignore = "нет записи: tools/arbitrum_record/pancake_stable.py"]
fn calldata_is_bot_abi() {
    for p in read("pools.json").as_array().unwrap() {
        let c = p["bot_calls"].as_array().unwrap();
        assert_eq!(c.len(), 8 * 2 + 3, "пул {}", p["pool"]);
        for k in 0..8 {
            let i = U256::from(k);
            assert_eq!(data(&c[2 * k]), IStableSwapV1::coinsCall { i }.abi_encode());
            assert_eq!(
                data(&c[2 * k + 1]),
                IStableSwapV1::balancesCall { i }.abi_encode()
            );
        }
        assert_eq!(data(&c[16]), IStableSwapV1::feeCall {}.abi_encode());
        assert_eq!(data(&c[17]), IStableSwapV1::ACall {}.abi_encode());
        assert_eq!(data(&c[18]), IPancakeStable::is_killedCall {}.abi_encode());
        assert_eq!(
            data(&p["get_dy"][0])[..4].to_vec(),
            IPancakeStable::get_dyCall::SELECTOR.to_vec()
        );
    }
}

/// Монеты пула по ответам бота (load_stable_u::head).
fn coins(p: &Value) -> (Vec<Address>, Vec<U256>) {
    let got: Vec<Result<Bytes, String>> = p["bot_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(answer)
        .collect();
    load_stable_u::head(&got, &bad).expect("head")
}

/// (2) + (3) реестр фабрики подтверждает пул (как verify_stable_u.rs); код пула — проверенный исходник.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/pancake_stable.py"]
fn registry_and_code() {
    let meta = read("meta.json");
    assert_eq!(
        meta["chainId"].as_str(),
        Some("0xa4b1"),
        "сеть — Arbitrum One"
    );
    let reg = read("factory.json");
    let listed: Vec<Address> = reg["swapPairContract"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| Address::from_slice(&answer(r).expect("swapPairContract")[12..]))
        .collect();
    for a in meta["api_pools"].as_array().unwrap() {
        assert!(
            listed.contains(&addr(a.as_str().unwrap())),
            "пул API {a} не в реестре"
        );
    }
    for p in read("pools.json").as_array().unwrap() {
        let pool = addr(p["pool"].as_str().unwrap());
        let (c, _) = coins(p);
        assert_eq!(c.len(), 2, "пул {pool}: монет {}", c.len());
        for r in p["getPairInfo"].as_array().unwrap() {
            let raw = answer(r).expect("getPairInfo");
            let info = IPancakeStableFactory::getPairInfoCall::abi_decode_returns(&raw).unwrap();
            assert_eq!(info.swapContract, pool, "реестр getPairInfo");
        }
        let tail = p["code"]["tail64"].as_str().unwrap();
        assert!(tail.ends_with(CBOR), "пул {pool}: код не тот ({tail})");
    }
}

/// get_dy пула (PancakeStableSwapTwoPool.sol, Sourcify 42161): комиссия после перевода в единицы монеты j.
fn get_dy(
    i: usize,
    j: usize,
    dx: U256,
    bal: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
) -> U256 {
    let p = precision();
    let xs = xp(bal, rates).unwrap();
    let x = xs[i] + dx * rates[i] / p;
    let y = get_y(i, j, x, &xs, amp).unwrap();
    let dy = (xs[j] - y - U256::from(1u8)) * p / rates[j];
    dy - fee * dy / fee_denominator()
}

/// (4) состояние разбором бота; get_dy пула = порт; при rates[j] = 1e18 — и exchange_out_t бота.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/pancake_stable.py"]
fn math_matches_get_dy() {
    let mut checked = 0;
    for p in read("pools.json").as_array().unwrap() {
        let got: Vec<Result<Bytes, String>> = p["bot_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(answer)
            .collect();
        let (c, bal) = load_stable_u::head(&got, &bad).unwrap();
        let rates: Vec<U256> = p["decimals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                rate_for_decimals(U256::from_be_slice(&answer(d).unwrap()).to::<u8>()).unwrap()
            })
            .collect();
        assert_eq!(rates.len(), c.len());
        // Ответы бота: 16 — fee(), дальше A(), is_killed() (load_stable_u::answers); пул на паузе бот не торгует,
        // математика та же.
        let state = load_stable_u::finish(&got, 16, false, &c, bal, rates, (0, 1), &bad).unwrap();
        let PoolState::StableSwap(st) = state else {
            panic!("не StableSwap");
        };
        for q in p["get_dy"].as_array().unwrap() {
            let Ok(raw) = answer(q) else { continue };
            let want = U256::from_be_slice(&raw);
            let (i, j) = (
                q["i"].as_u64().unwrap() as usize,
                q["j"].as_u64().unwrap() as usize,
            );
            let dx: U256 = q["dx"].as_str().unwrap().parse().unwrap();
            let ours = get_dy(i, j, dx, &st.balances, &st.rates, st.amp, st.fee);
            assert_eq!(ours, want, "пул {} {i}→{j} dx {dx}", p["pool"]);
            if st.rates[j] == precision() {
                let ex = exchange_out_t(
                    i,
                    j,
                    dx,
                    &st.balances,
                    &st.rates,
                    st.amp,
                    st.fee,
                    &curve2020(),
                );
                assert_eq!(ex, Some(want), "exchange при 18 знаках, пул {}", p["pool"]);
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "ни одного ответа get_dy в записи");
}

/// (5) TokenExchange из логов пулов разбирается ABI бота.
#[test]
#[ignore = "нет записи: tools/arbitrum_record/pancake_stable.py"]
fn token_exchange_decodes() {
    let logs = read("logs.json");
    let Some(all) = logs["logs"].as_array() else {
        panic!("логов нет: {}", logs["error"]);
    };
    let mut n = 0;
    for l in all {
        let topics: Vec<B256> = l["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().parse().unwrap())
            .collect();
        if topics.first() != Some(&IPancakeStable::TokenExchange::SIGNATURE_HASH) {
            continue;
        }
        let d = Bytes::from(hex::decode(l["data"].as_str().unwrap()).unwrap());
        let ev = IPancakeStable::TokenExchange::decode_log_data(&LogData::new_unchecked(topics, d));
        assert!(
            ev.is_ok(),
            "TokenExchange {}: {:?}",
            l["transactionHash"],
            ev.err()
        );
        n += 1;
    }
    eprintln!("TokenExchange в окне: {n}");
}
