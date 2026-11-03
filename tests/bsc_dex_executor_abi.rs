//! Файл: tests/bsc_dex_executor_abi.rs
//! Что делает: сверка Rust-стороны исполнителя ArbExecutor (решение 122, часть X) с артефактами сборки контракта в
//! репозитории (contracts/out/, собраны contracts/build.sh из contracts/ArbExecutor.sol):
//! - ABI-кодирование шагов и вызова execute (тип из alloy sol! по ArbExecutor.abi) совпадает с ABI контракта: селектор
//!   и подпись execute из JSON ABI, раскладка кортежа Step (поля и типы по порядку), кодирование → разбор без потерь;
//! - номера видов шагов (kind.rs Params::kind) = константам контракта (uint8 internal constant … = N в ArbExecutor.sol);
//! - раскладка data вида: V2 — uint32 комиссии, CURVE — (int128 i, int128 j), STABLE_U256 — (uint256 i, uint256 j),
//!   SADDLE — (uint8 i, uint8 j), STABLE_U256_BNB — (uint256 i, uint256 j, bool bnbIn), остальные — пусто; селекторы
//!   обмена новых видов — по официальным подписям; шаг с нативным BNB пула — receive() только от bnbFrom, WBNB
//!   withdraw/deposit в коде, BNB после маршрута — владельцу (решение «122 X (нативный BNB в стейблсвапах)»);
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
        ("STABLE_U256", Params::StableU256 { i: 0, j: 1 }),
        ("SADDLE", Params::Saddle { i: 0, j: 1 }),
        (
            "STABLE_U256_BNB",
            Params::StableU256Bnb {
                i: 0,
                j: 1,
                bnb_in: true,
            },
        ),
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
    // STABLE_U256 — abi.encode(uint256 i, uint256 j); SADDLE — abi.encode(uint8 i, uint8 j): по слову на индекс.
    let u = Params::StableU256 { i: 1, j: 0 }.data();
    assert_eq!(
        <(U256, U256)>::abi_decode_params(&u).unwrap(),
        (U256::from(1u8), U256::ZERO)
    );
    // STABLE_U256_BNB — abi.encode(uint256 i, uint256 j, bool bnbIn): три слова.
    let b = Params::StableU256Bnb {
        i: 1,
        j: 0,
        bnb_in: true,
    }
    .data();
    assert_eq!(b.len(), 96);
    assert_eq!(
        <(U256, U256, bool)>::abi_decode_params(&b).unwrap(),
        (U256::from(1u8), U256::ZERO, true)
    );
    let b = Params::StableU256Bnb {
        i: 0,
        j: 1,
        bnb_in: false,
    }
    .data();
    assert_eq!(
        <(U256, U256, bool)>::abi_decode_params(&b).unwrap(),
        (U256::ZERO, U256::from(1u8), false)
    );
    let sd = Params::Saddle { i: 2, j: 1 }.data();
    assert_eq!(sd.len(), 64);
    assert_eq!((sd[31], sd[63]), (2, 1));
    assert!(sd[..31].iter().chain(&sd[32..63]).all(|b| *b == 0));
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

/// Ревью стейблсвапов: шаг StableSwap (CURVE / STABLE_U256 / SADDLE) с нативной монетой 0xEeee… не кодируется; в
/// контракте — один путь _stable для всех трёх (approve → обмен → «out» → разрешение 0) и запрет цикла из долей («loop»).
#[test]
fn stable_native_refused_and_contract_rules_present() {
    for params in [
        Params::Curve { i: 0, j: 1 },
        Params::StableU256 { i: 0, j: 1 },
        Params::Saddle { i: 0, j: 1 },
        Params::StableU256Bnb {
            i: 0,
            j: 1,
            bnb_in: false,
        },
    ] {
        let spec = StepSpec {
            params,
            pool: POOL,
            token_in: kind::STABLE_NATIVE,
            token_out: USDT,
            share_bps: ALL_BPS,
        };
        assert!(kind::encode(&spec).is_err(), "{params:?}");
        let ok = StepSpec {
            token_in: WBNB,
            ..spec
        };
        assert!(kind::encode(&ok).is_ok(), "{params:?}");
    }
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/ArbExecutor.sol"
    ))
    .unwrap();
    assert!(src.contains("} else if (k == CURVE || k == STABLE_U256 || k == SADDLE) {"));
    assert!(src.contains("\"loop\")"));
}

/// Нативный BNB пула (решение «122 X (нативный BNB в стейблсвапах)»): в контракте — свой путь STABLE_U256_BNB, приём
/// BNB только от bnbFrom (transient), BNB после маршрута — владельцу; в ABI — receive (payable), execute — не payable;
/// в коде в сети — селекторы WBNB withdraw(uint256) / deposit() (gnosis/canonical-weth@0dd1ea3e WETH9.sol L25-L42).
#[test]
fn stable_bnb_contract_rules_present() {
    use alloy::json_abi::StateMutability;
    use alloy_primitives::keccak256;
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/ArbExecutor.sol"
    ))
    .unwrap();
    for must in [
        "} else if (k == STABLE_U256_BNB) {",
        "receive() external payable {\n        require(msg.sender == bnbFrom, \"bnb\");\n    }",
        "address transient bnbFrom;",
        "bnbFrom = s.tokenIn;",
        "bnbFrom = s.pool;",
        "IStableSwapU256(s.pool).exchange{value: amount}(i, j, amount, 0);",
        "IWBNB(s.tokenOut).deposit{value: address(this).balance}();",
        "_sweepBnb();",
    ] {
        assert!(src.contains(must), "нет в контракте: {must}");
    }
    // bnbFrom обнуляется после каждого вызова, где BNB ждётся (две установки — два обнуления).
    assert_eq!(src.matches("bnbFrom = address(0);").count(), 2);
    let abi = abi();
    assert_eq!(
        abi.receive.as_ref().map(|r| r.state_mutability),
        Some(StateMutability::Payable)
    );
    assert!(abi.fallback.is_none());
    assert_eq!(
        abi.function("execute").unwrap()[0].state_mutability,
        StateMutability::NonPayable
    );
    let runtime = code::runtime().unwrap();
    for sig in ["withdraw(uint256)", "deposit()"] {
        let mut push = vec![0x63u8];
        push.extend(&keccak256(sig.as_bytes())[..4]);
        assert!(
            runtime.windows(5).any(|w| w == push.as_slice()),
            "{sig} в коде"
        );
    }
    assert_eq!(
        &keccak256(b"withdraw(uint256)")[..4],
        [0x2e, 0x1a, 0x7d, 0x4d]
    );
    assert_eq!(&keccak256(b"deposit()")[..4], [0xd0, 0xe3, 0x0d, 0xb0]);
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

/// Селекторы обмена новых видов (решение 122 X, стейблсвапы) — keccak официальных подписей: PancakeSwap StableSwap
/// (pancake-smart-contracts@cb079908 PancakeStableSwapTwoPool.sol L425) и Lista (moolah@eb7d73a StableSwapPool.sol
/// L478) — exchange(uint256,uint256,uint256,uint256) = 0x5b41b908; Nerve (nerve-finance/contracts@bda7578 Swap.sol
/// L389) — swap(uint8,uint8,uint256,uint256,uint256). Те же подписи — в contracts/IPools.sol, селекторы — в коде в сети
/// (PUSH4 0x63 + селектор).
#[test]
fn stable_kinds_call_official_selectors() {
    use alloy_primitives::keccak256;
    let sel = |sig: &str| keccak256(sig.as_bytes())[..4].to_vec();
    assert_eq!(
        sel("exchange(uint256,uint256,uint256,uint256)"),
        [0x5b, 0x41, 0xb9, 0x08]
    );
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/IPools.sol"))
        .unwrap();
    assert!(src.contains(
        "function exchange(uint256 i, uint256 j, uint256 dx, uint256 minDy) external payable;"
    ));
    assert!(src.contains(
        "function swap(uint8 tokenIndexFrom, uint8 tokenIndexTo, uint256 dx, uint256 minDy, uint256 deadline)"
    ));
    let runtime = code::runtime().unwrap();
    for sig in [
        "exchange(uint256,uint256,uint256,uint256)",
        "swap(uint8,uint8,uint256,uint256,uint256)",
        "exchange(int128,int128,uint256,uint256)",
    ] {
        let mut push = vec![0x63u8];
        push.extend(sel(sig));
        assert!(
            runtime.windows(5).any(|w| w == push.as_slice()),
            "{sig} в коде"
        );
    }
}
