//! Файл: tests/bsc_dex_executor_abi.rs
//! Что делает: сверка Rust-стороны исполнителя ArbExecutor (решение 122, часть X) с артефактами сборки контракта в
//! репозитории (contracts/out/, собраны contracts/build.sh из contracts/ArbExecutor.sol):
//! - ABI-кодирование шагов и вызова execute (тип из alloy sol! по ArbExecutor.abi) совпадает с ABI контракта: селектор
//!   и подпись execute из JSON ABI, раскладка кортежа Step (поля и типы по порядку), кодирование → разбор без потерь;
//! - номера видов шагов (kind.rs Params::kind) = константам контракта (uint8 internal constant … = N в ArbExecutor.sol);
//! - раскладка data вида: V2 — uint32 комиссии, CURVE — (int128 i, int128 j), остальные — пусто;
//! - хеши артефактов совпадают с ArbExecutor.sha256, код в сети — хвост кода создания, слот владельца — из раскладки.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::executor::{abi, kind, code}, contracts/*.

use alloy::json_abi::JsonAbi;
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, U256, address};

use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::code;
use arb::venues::bsc_dex::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};

/// Реальные адреса BSC: USDT (BEP20) и WBNB — справочные адреса токенов сети; пул — PancakeSwap V2 WBNB/USDT
/// (тот же, что в записи маршрута tests/data/routes_2026-10-29, route_common::WBNB_USDT_V2).
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const POOL: Address = address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae");

fn abi() -> JsonAbi {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/out/ArbExecutor.abi"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn execute_selector_and_step_layout_match_contract_abi() {
    let abi = abi();
    let f = &abi.function("execute").expect("execute в ABI")[0];
    assert_eq!(f.selector(), ArbExecutor::executeCall::SELECTOR);
    assert_eq!(f.signature(), ArbExecutor::executeCall::SIGNATURE);
    assert_eq!(
        f.signature(),
        "execute(address,uint256,address,uint256,uint256,(uint8,address,address,address,uint16,bytes)[])"
    );
    let step = &f.inputs[5].components;
    let names: Vec<&str> = step.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["kind", "pool", "tokenIn", "tokenOut", "shareBps", "data"]
    );
    // Обратные вызовы — по официальным именам и подписям пулов (contracts/IPools.sol).
    for sig in [
        "uniswapV3SwapCallback(int256,int256,bytes)",
        "pancakeV3SwapCallback(int256,int256,bytes)",
        "algebraSwapCallback(int256,int256,bytes)",
        "swapCallback(int256,int256,bytes)",
        "maverickV2SwapCallback(address,uint256,uint256,bytes)",
    ] {
        let name = sig.split('(').next().unwrap();
        assert_eq!(abi.function(name).expect(sig)[0].signature(), sig);
    }
}

#[test]
fn kinds_match_contract_constants() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/ArbExecutor.sol"
    ))
    .unwrap();
    let constant = |name: &str| -> u8 {
        let pat = format!("uint8 internal constant {name} = ");
        let rest = &src[src
            .find(&pat)
            .unwrap_or_else(|| panic!("{name} нет в контракте"))
            + pat.len()..];
        rest[..rest.find(';').unwrap()].parse().unwrap()
    };
    let all = [
        ("V2", Params::V2 { fee_pips: 2500 }),
        ("V3", Params::V3),
        ("ALGEBRA", Params::Algebra),
        ("SOLIDLY", Params::Solidly),
        ("CURVE", Params::Curve { i: 0, j: 1 }),
        ("DODO", Params::Dodo),
        ("WOO", Params::Woo),
        ("MAVERICK", Params::Maverick),
        ("ELASTIC", Params::Elastic),
    ];
    for (name, p) in all {
        assert_eq!(p.kind(), constant(name), "вид {name}");
    }
}

#[test]
fn step_data_layouts() {
    let v2 = Params::V2 { fee_pips: 2500 }.data();
    assert_eq!(v2.len(), 32);
    assert_eq!(u32::abi_decode(&v2).unwrap(), 2500);
    let c = Params::Curve { i: 1, j: 0 }.data();
    assert_eq!(<(i128, i128)>::abi_decode_params(&c).unwrap(), (1, 0));
    for p in [
        Params::V3,
        Params::Algebra,
        Params::Solidly,
        Params::Dodo,
        Params::Woo,
        Params::Maverick,
        Params::Elastic,
    ] {
        assert!(p.data().is_empty());
    }
}

#[test]
fn encode_round_trips_and_rejects_bad_steps() {
    let spec = StepSpec {
        params: Params::V2 { fee_pips: 2500 },
        pool: POOL,
        token_in: USDT,
        token_out: WBNB,
        share_bps: 6000,
    };
    let s = kind::encode(&spec).unwrap();
    let call = ArbExecutor::executeCall {
        tokenIn: USDT,
        amountIn: U256::from(40u64) * U256::from(10u64).pow(U256::from(18u64)),
        tokenOut: WBNB,
        minOut: U256::from(1u64),
        deadline: U256::from(1_800_000_000u64),
        steps: vec![s.clone()],
    };
    let data = call.abi_encode();
    assert_eq!(&data[..4], ArbExecutor::executeCall::SELECTOR.as_slice());
    assert_eq!(ArbExecutor::executeCall::abi_decode(&data).unwrap(), call);
    assert_eq!((s.kind, s.shareBps), (0, 6000));
    for bad in [0u16, ALL_BPS + 1] {
        assert!(
            kind::encode(&StepSpec {
                share_bps: bad,
                ..spec.clone()
            })
            .is_err()
        );
    }
    assert!(
        kind::encode(&StepSpec {
            token_out: USDT,
            ..spec.clone()
        })
        .is_err()
    );
    assert!(
        kind::encode(&StepSpec {
            params: Params::V2 {
                fee_pips: 1_000_000
            },
            ..spec
        })
        .is_err()
    );
}

#[test]
fn artifacts_match_recorded_hashes() {
    let bin = code::creation().expect("код создания сверен");
    let runtime = code::runtime().expect("код в сети сверен");
    assert!(!runtime.is_empty() && bin.len() > runtime.len());
    // Код создания = конструктор + код в сети (solc кладёт deployedBytecode в хвост кода создания).
    assert!(bin.windows(runtime.len()).any(|w| w == runtime.as_ref()));
    assert_eq!(code::owner_slot().unwrap(), B256::ZERO);
    assert!(code::solc().starts_with("0.8.37+"));
    assert_eq!(code::creation_sha256().len(), 64);
}
