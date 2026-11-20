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
//! - решение 457: сборка contracts/out/ — v4 (via-IR, solc 0.8.37): SHA-256 кода в сети, размер ≤ 24 576 байт
//!   (EIP-170), функции и селекторы шагов 22–25 в ABI и коде; селекторы внешних вызовов — exec_callback::has_selector
//!   (PUSH4 или форма via-IR PUSHn (селектор >> k) PUSH1 (0xe0 + k) SHL).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::executor::{abi, kind, code}, pools::exec_callback, contracts/*.

use alloy::json_abi::JsonAbi;
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, B256, U256, address};

use arb::venues::bsc_dex::exec::executor::abi::ArbExecutor;
use arb::venues::bsc_dex::exec::executor::code;
use arb::venues::bsc_dex::exec::executor::kind::{self, ALL_BPS, Params, StepSpec};
use arb::venues::bsc_dex::pools::exec_callback::has_selector;

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
        // Решение 284: пара Nomiswap Stable — только v2.
        ("NOMI_STABLE", Params::NomiStable),
        // Решение 285: пул Metric OMM — только v2.
        ("METRIC", Params::Metric { zero_for_one: true }),
        // Решение 286: пул iZiSwap / Biswap V3 — только v2.
        ("IZI", Params::Izi),
        // Решение 287: пул KyberSwap Classic DMM — только v2.
        ("KYBER_DMM", Params::KyberDmm),
        // Решение 288: пара SmarDex — только v2.
        ("SMARDEX", Params::Smardex),
        // Решение 289: пул Balancer V2 — только v2.
        ("BALANCER", Params::Balancer { pool_id: B256::ZERO }),
        // Решение 290: пул Liquidity Book V2.1 — только v2.
        ("LB", Params::Lb),
        // Решение 395 (t181): пулы внутри менеджеров — только v3.
        ("UNI_V4", Params::UniV4 { key: v4_key() }),
        ("INF_CL", Params::InfCl { key: inf_key() }),
        ("INF_BIN", Params::InfBin { key: inf_key() }),
        // Решение 440: пул Fluid DEX T1 — только v4.
        ("FLUID", Params::Fluid(fluid_step(1))),
        // Решение 441: пул Balancer V3 — только v4.
        ("BALANCER_V3", Params::BalancerV3 { pool: Address::ZERO }),
        // Решение 474: пул Balancer V3 через буфер ERC4626 — шаг 27 (сборка v5 не собрана).
        (
            "BALANCER_V3_BUF",
            Params::BalancerV3Buf {
                pool: Address::ZERO,
                w_in: POOL,
                w_out: Address::ZERO,
            },
        ),
        // Решение 445: пул Liquidity Book v2.0 — шаг 24 (решение 456; решение 457 — в сборке v4 out/).
        ("LB20", Params::Lb20),
        // Решение 447: пул Ekubo в ядре Core — шаг 25 (решение 456; решение 457 — в сборке v4 out/).
        (
            "EKUBO",
            Params::Ekubo {
                config: B256::ZERO,
                native: false,
                forward: false,
            },
        ),
        // Решение 470 (T470-06): пул Aero Basic — шаг 26, только v5 (в исходнике; сборки v5 нет).
        ("AERO_BASIC", Params::AeroBasic),
        // Решение 472 (T472-08): пул Curve с нативным ETH — шаг 28, только v5 (в исходнике; сборки v5 нет).
        (
            "CURVE_ETH",
            Params::CurveEth {
                i: 0,
                j: 1,
                eth_in: true,
            },
        ),
        // Решение 484: пул Wombat Exchange — шаг 30, только v5 (в исходнике; сборки v5 нет).
        ("WOMBAT", Params::Wombat),
        // Решение 485: скважина Basin — шаг 29, только v5 (в исходнике; сборки v5 нет).
        ("BASIN", Params::Basin),
    ];
    for (name, p) in all {
        assert_eq!(p.kind(), constant(name), "вид {name}");
    }
}

/// Слой ликвидности Fluid в Arbitrum One (Instadapp/fluid-contracts-public@404d73f deployments/arbitrum/Liquidity.json).
const FLUID_LIQUIDITY: Address = address!("0x52Aa899454998Be5b000Ad077a46Bbe360F4e497");

/// Шаг FLUID (решение 440) с данным видом нативной монеты.
fn fluid_step(native: u8) -> kind::FluidStep {
    kind::FluidStep {
        zero_for_one: true,
        native,
        liquidity: FLUID_LIQUIDITY,
    }
}

/// Ключи для сверки номеров видов (решение 395): содержимое не важно — только номер вида.
fn v4_key() -> kind::V4Key {
    kind::V4Key {
        currency0: USDT,
        currency1: WBNB,
        fee: 3000,
        tick_spacing: alloy_primitives::aliases::I24::try_from(60i32).unwrap(),
        hooks: Address::ZERO,
    }
}

fn inf_key() -> kind::InfKey {
    kind::InfKey {
        currency0: USDT,
        currency1: WBNB,
        hooks: Address::ZERO,
        manager: POOL,
        fee: 3000,
        parameters: B256::ZERO,
    }
}

#[test]
fn step_data_layouts() {
    let v2 = Params::V2 { fee_pips: 2500 }.data();
    assert_eq!(v2.len(), 32);
    assert_eq!(u32::abi_decode(&v2).unwrap(), 2500);
    let c = Params::Curve { i: 1, j: 0 }.data();
    assert_eq!(<(i128, i128)>::abi_decode_params(&c).unwrap(), (1, 0));
    // Решение 472: CURVE_ETH — abi.encode(int128 i, int128 j, bool ethIn): три слова.
    let ce = Params::CurveEth {
        i: 0,
        j: 2,
        eth_in: false,
    }
    .data();
    assert_eq!(ce.len(), 96);
    assert_eq!(
        <(i128, i128, bool)>::abi_decode_params(&ce).unwrap(),
        (0, 2, false)
    );
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
    // BALANCER (решение 289) — abi.encode(bytes32 poolId): одно слово, id как есть.
    let id = B256::repeat_byte(0x7e);
    let bl = Params::Balancer { pool_id: id }.data();
    assert_eq!(bl.len(), 32);
    assert_eq!(B256::abi_decode(&bl).unwrap(), id);
    // FLUID (решение 440) — abi.encode(bool swap0to1, uint8 native, address liquidity): три слова.
    let fl = Params::Fluid(fluid_step(2)).data();
    assert_eq!(fl.len(), 96);
    assert_eq!(
        <(bool, U256, Address)>::abi_decode_params(&fl).unwrap(),
        (true, U256::from(2u8), FLUID_LIQUIDITY)
    );
    // BALANCER_V3_BUF (решение 474) — abi.encode(address пула, address wIn, address wOut): три слова.
    let bb = Params::BalancerV3Buf {
        pool: POOL,
        w_in: USDT,
        w_out: Address::ZERO,
    }
    .data();
    assert_eq!(bb.len(), 96);
    assert_eq!(
        <(Address, Address, Address)>::abi_decode_params(&bb).unwrap(),
        (POOL, USDT, Address::ZERO)
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
        Params::AeroBasic,
        Params::Wombat,
        Params::Basin,
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
        // Решение 472 (T472-08): шаг CURVE_ETH — тот же порядок для пула Curve с нативным ETH.
        "} else if (k == CURVE_ETH) {",
        "ICurveEthPool(s.pool).exchange{value: amount}(i, j, amount, 0);",
        "IWBNB(s.tokenOut).deposit{value: address(this).balance - ethHad}();",
        // Ревью 272 (А3): в обёртку — только BNB, пришедший от пула этим шагом (разность баланса).
        "IWBNB(s.tokenOut).deposit{value: address(this).balance - bnbHad}();",
        "_sweepBnb();",
    ] {
        assert!(src.contains(must), "нет в контракте: {must}");
    }
    // bnbFrom обнуляется после каждого вызова, где BNB ждётся: сколько установок — столько обнулений (stable BNB —
    // две, _unwrapAll executeNative — одна, решение 272; шаг FLUID — две, решение 440: вход и выход в нативной монете;
    // шаг EKUBO — две, решение 454: нативный вход и выход; решение 456 — слияние: 3 + 2 + 2 = 7; шаг CURVE_ETH — две,
    // решение 472: вход и выход в нативном ETH — 9; шаги v5 AERO_BASIC = 26 и BALANCER_V3_BUF = 27 нативной монеты не
    // ждут — слияние t247: по итоговому contracts/ArbExecutor.sol 9 установок и 9 сбросов; шаг WOMBAT = 30, решение 484,
    // нативной монеты не ждёт — по факту исходника после правки установок и сбросов по-прежнему 9). Только строки кода
    // (комментарии тоже содержат «bnbFrom = …»).
    let code: Vec<&str> =
        src.lines().map(str::trim).filter(|l| l.starts_with("bnbFrom = ")).collect();
    let resets = code.iter().filter(|l| **l == "bnbFrom = address(0);").count();
    let sets = code.len() - resets;
    assert_eq!(resets, 9);
    assert_eq!(sets, resets);
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
    // Решение 457: код v4 (via-IR) — селектор внешнего вызова PUSH4 или в форме via-IR (has_selector).
    let runtime = code::runtime().unwrap();
    for sig in ["withdraw(uint256)", "deposit()"] {
        assert!(
            has_selector(&runtime, &keccak256(sig.as_bytes())[..4]),
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
    // Ревью 272 (А1): обёртка из конструктора — слот 1 (owner остаётся в слоте 0 — сверка onchain.rs и сухая сделка).
    assert_eq!(code::wnative_slot().unwrap(), B256::with_last_byte(1));
    // Код транзакции создания = код создания + адрес обёртки словом ABI (32 байта, адрес справа).
    let w = alloy_primitives::Address::repeat_byte(0xbb);
    let full = code::creation_with(w).unwrap();
    assert_eq!(&full[..bin.len()], bin.as_ref());
    assert_eq!(
        &full[bin.len()..],
        B256::left_padding_from(w.as_slice()).as_slice()
    );
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
        assert!(has_selector(&runtime, &sel(sig)), "{sig} в коде");
    }
}

/// Решение 457: сборка contracts/out/ — исполнитель v4 конвейером via-IR (solc 0.8.37, оптимизатор 200 прогонов):
/// SHA-256 кода в сети, размер ≤ 24 576 байт (EIP-170 — https://eips.ethereum.org/EIPS/eip-170), обратные вызовы шагов
/// 23 и 25 в ABI, вызовы шагов 22–25 в коде; в замороженном v3 (contracts/out/v3/) их нет.
#[test]
fn v4_build_size_hash_and_step_selectors() {
    use alloy_primitives::keccak256;
    let rt = code::runtime().unwrap();
    assert_eq!(rt.len(), 21_775, "размер кода в сети v4");
    assert!(rt.len() <= 24_576, "EIP-170");
    assert_eq!(
        code::runtime_sha256(),
        Some("2bf86e58e5719f88d709bcf06350570cbd60318f1e49e9f9d3a9c85c3ee10762")
    );
    assert_eq!(code::current_version().unwrap(), code::Version::V4);
    let abi = abi();
    for sig in ["balancerV3Callback(bytes)", "locked_6416899205(uint256)"] {
        let name = sig.split('(').next().unwrap();
        assert_eq!(abi.function(name).expect(sig)[0].signature(), sig);
    }
    let v3 = code::runtime_v3().unwrap();
    // FLUID — swapIn (IFluidDexT1), BALANCER_V3 — balancerV3Callback, LB20 — tokenX() пары v2.0, EKUBO —
    // completePayments() ядра (селекторы — подписи contracts/IPools.sol).
    for sig in [
        "swapIn(bool,uint256,uint256,address)",
        "balancerV3Callback(bytes)",
        "tokenX()",
        "completePayments()",
    ] {
        let s = &keccak256(sig.as_bytes())[..4];
        assert!(has_selector(&rt, s), "{sig} в коде v4");
        assert!(!has_selector(&v3, s), "{sig} — нет в коде v3");
    }
}

/// Решение 474: вызова erc4626BufferWrapOrUnwrap (метка шага BALANCER_V3_BUF = 27, kind_buffer::BUFFER_MARK, селектор
/// 0x43583be5) нет ни в одной собранной версии исполнителя (out/, out/v3/, out/v2/, out/v1/) — шаг 27 бот не строит,
/// пока нет сборки v5. Исходник contracts/ArbExecutor.sol шаг содержит.
#[test]
fn buffer_step_only_in_source() {
    use alloy_primitives::keccak256;
    use arb::venues::bsc_dex::exec::executor::kind_buffer::BUFFER_MARK;
    let sel = &keccak256(BUFFER_MARK.as_bytes())[..4];
    assert_eq!(sel, [0x43, 0x58, 0x3b, 0xe5]);
    for rt in [
        code::runtime().unwrap(),
        code::runtime_v3().unwrap(),
        code::runtime_v2().unwrap(),
        code::runtime_v1().unwrap(),
    ] {
        assert!(!has_selector(&rt, sel));
    }
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/ArbExecutor.sol"
    ))
    .unwrap();
    assert!(src.contains("IBalancerV3Vault(v).erc4626BufferWrapOrUnwrap("));
}

/// SHA-256 кода в сети исполнителя v4 (contracts/README.md, решение 457; contracts/out/ArbExecutor.sha256 сборки v4).
const V4_RUNTIME_SHA: &str = "2bf86e58e5719f88d709bcf06350570cbd60318f1e49e9f9d3a9c85c3ee10762";

/// Ревью t244 (решение 474): пересборка исполнителя (v5, шаг 27) не должна потерять код v4 — по нему бот сверяет
/// развёрнутые кошельки v4. Сейчас contracts/out/ = v4 (тест проходит сразу); если out/ ≠ v4 — обязателен out/v4/ с
/// кодом v4 и строкой хеша в его ArbExecutor.sha256 (иначе красный CI). Хеш v4 записан и в contracts/README.md.
#[test]
fn v4_kept_after_rebuild() {
    use sha2::{Digest, Sha256};
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/out");
    let sha = |p: &str| {
        std::fs::read_to_string(p)
            .ok()
            .map(|t| alloy_primitives::hex::encode(Sha256::digest(t.as_bytes())))
    };
    let readme = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/README.md"
    ))
    .unwrap();
    assert!(readme.contains(V4_RUNTIME_SHA), "хеш v4 в contracts/README.md");
    if sha(&format!("{out}/ArbExecutor.bin-runtime")).as_deref() == Some(V4_RUNTIME_SHA) {
        return;
    }
    let v4 = format!("{out}/v4");
    assert_eq!(
        sha(&format!("{v4}/ArbExecutor.bin-runtime")).as_deref(),
        Some(V4_RUNTIME_SHA),
        "out/ пересобран — код v4 должен лежать в contracts/out/v4/"
    );
    let rec = std::fs::read_to_string(format!("{v4}/ArbExecutor.sha256")).unwrap_or_default();
    assert!(rec.contains(&format!("{V4_RUNTIME_SHA}  ArbExecutor.bin-runtime")));
}
