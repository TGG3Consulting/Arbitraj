//! Файл: tests/arbitrum_aero_live.rs
//! Что проверяет: решение 470 (T451-01/02) — Aero Basic (aero_basic) и Aero Slipstream V3 (slipstream_aero).
//! Без записи: вид и вариант по dex.kind, шаг исполнителя (SOLIDLY / V3).
//! На записи своего узла (tests/data/arbitrum_aero_<дата>/record.json, tools/arbitrum_record/aero.py; #[ignore] до
//! записи): наша математика пула Basic (комиссия getBaseFee → math/solidly_fee net_in → x·y=k math/v2 или
//! x³y+xy³ math/swap_solidly) = getAmountOut пула на том же блоке на каждую записанную сумму; кривая — POOL_TYPE;
//! реестр фабрики getPool(t0, t1) = пул; у пулов CL крючок getPoolSwapHook = официальный, fee() ≤ 100000.
//! Кто вызывает: cargo test (CI).
//! От чего зависит: arb::venues::bsc_dex::{math::{v2, swap_solidly, solidly_fee}, pools::{families, exec_kind,
//! family_variant, abi_aero_basic}}, core/types (SolidlyFee, FeeRule).

use alloy_primitives::{Address, B256, U256};
use serde_json::Value;

use arb::core::types::{FeeRule, PoolKind, SolidlyFee};
use arb::venues::bsc_dex::exec::executor::code::{self, Version};
use arb::venues::bsc_dex::exec::executor::kind::Params;
use arb::venues::bsc_dex::math::{solidly_fee, swap_solidly, v2};
use arb::venues::bsc_dex::pools::abi_aero_basic::{stable_type, volatile_type};
use arb::venues::bsc_dex::pools::exec_callback;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::family_variant::Variant;

const NO_RECORD: &str = "нет записи: tools/arbitrum_record/aero.py";

/// Вид, вариант и шаг исполнителя по dex.kind (исходники — шапки pools/abi_aero_basic.rs и pools/aero_cl.rs).
#[test]
fn kinds_and_steps() {
    let (pk, proto, graphable, ..) = families::spec("aero_basic").unwrap();
    assert_eq!((pk, proto, graphable), (PoolKind::Solidly, Proto::AeroBasic, true));
    assert_eq!(exec_kind::of("aero_basic"), Some(ExecKind::Solidly));
    let (pk, proto, ..) = families::spec("slipstream_aero").unwrap();
    assert_eq!((pk, proto), (PoolKind::V3, Proto::Slipstream));
    assert_eq!(exec_kind::of("slipstream_aero"), Some(ExecKind::V3));
    let f = families::make("aero-slipstream-arbitrum@arbitrum", "slipstream_aero").unwrap();
    assert_eq!(f.variant, Variant::SlipstreamAero);
    let f = families::make("slipstream_bsc", "slipstream").unwrap();
    assert_eq!(f.variant, Variant::Base);
}

/// Решение 470 (T470-06): шаг AERO_BASIC = 26 — только исполнитель v5 (решение 488: сборка v5 есть; до записи приёмки и
/// развёртывания шаг не исполняется); вид семейства aero_basic — SOLIDLY, у сверенного v5 — AERO_BASIC, у v4 и ниже — SOLIDLY.
#[test]
fn aero_basic_step_needs_v5() {
    assert_eq!(Params::AeroBasic.kind(), 26);
    assert!(Params::AeroBasic.data().is_empty());
    assert_eq!(exec_callback::since(ExecKind::AeroBasic), Version::V5);
    let why = exec_callback::step_absent(ExecKind::AeroBasic, Version::V4).expect("у v4 шага нет");
    assert!(why.contains("AERO_BASIC") && why.contains("v5"), "{why}");
    assert!(exec_callback::step_absent(ExecKind::AeroBasic, Version::V5).is_none());
    assert_eq!(Version::parse("v5"), Some(Version::V5));
    // Решение 488: сборка v5 есть — runtime_of(V5) даёт её код (для поиска селекторов шагов v5).
    assert!(code::runtime_of(Version::V5).is_ok());
    let f = families::make("aero-basic-volatile@arbitrum", "aero_basic").unwrap();
    assert_eq!(f.executor_kind, Some(ExecKind::Solidly));
    let v5 = exec_kind::for_version(f, Some(Version::V5));
    assert_eq!(v5, Some(ExecKind::AeroBasic));
    assert_eq!(exec_kind::for_version(f, Some(Version::V4)), Some(ExecKind::Solidly));
    assert_eq!(exec_kind::for_version(f, None), Some(ExecKind::Solidly));
    let cl = families::make("aero-slipstream-arbitrum@arbitrum", "slipstream_aero").unwrap();
    assert_eq!(exec_kind::for_version(cl, Some(Version::V5)), Some(ExecKind::V3));
}

fn record() -> Value {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let mut found: Vec<String> = std::fs::read_dir(base)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("arbitrum_aero_"))
        .collect();
    found.sort();
    let dir = format!("{base}{}/record.json", found.last().expect(NO_RECORD));
    serde_json::from_str(&std::fs::read_to_string(dir).unwrap()).unwrap()
}

/// Слово i ответа eth_call (hex). None — откат или короткий ответ.
fn w(v: &Value, i: usize) -> Option<U256> {
    let s = v.get("result")?.as_str()?.strip_prefix("0x")?;
    U256::from_str_radix(s.get(i * 64..(i + 1) * 64)?, 16).ok()
}

fn addr(u: U256) -> Address {
    Address::from_word(B256::from(u))
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/aero.py"]
fn basic_math_equals_pool_get_amount_out() {
    let r = record();
    let mut checked = 0;
    for (side, want_stable) in [("volatile", false), ("stable", true)] {
        for p in r[side]["pools"].as_array().unwrap() {
            let m = &p["metadata"];
            let word = |i| w(m, i).unwrap();
            let (dec0, dec1, r0, r1) = (word(0), word(1), word(2), word(3));
            let t0 = addr(word(4));
            let kind = w(&p["POOL_TYPE"], 0).unwrap();
            let stable = kind == U256::from_be_bytes(stable_type().0);
            assert!(stable || kind == U256::from_be_bytes(volatile_type().0));
            assert_eq!(stable, want_stable, "кривая пула {} — фабрика {side}", p["pool"]);
            let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
            assert_eq!(addr(w(&p["getPool"], 0).unwrap()), pool, "реестр getPool(t0, t1)");
            let fee = SolidlyFee {
                value: w(&p["getBaseFee"], 0).unwrap().to::<u32>(),
                rule: FeeRule::Numerator { denominator: 10_000 },
            };
            for q in p["quotes"].as_array().unwrap() {
                let Some(want) = w(&q["out"], 0) else { continue };
                let amount: U256 = q["amountIn"].as_str().unwrap().parse().unwrap();
                let tin: Address = q["tokenIn"].as_str().unwrap().parse().unwrap();
                let (ri, ro, di, dout) = if tin == t0 {
                    (r0, r1, dec0, dec1)
                } else {
                    (r1, r0, dec1, dec0)
                };
                let net = solidly_fee::net_in(amount, &fee).unwrap();
                let got = if stable {
                    swap_solidly::get_amount_out(net, ri, ro, di, dout, U256::ZERO).unwrap()
                } else {
                    v2::amount_out(net, ri, ro, (U256::from(1u8), U256::from(1u8))).unwrap()
                };
                assert_eq!(got, want, "пул {pool} вход {amount} {tin}");
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "в записи нет котировок пулов Basic");
}

#[test]
#[ignore = "нет записи: tools/arbitrum_record/aero.py"]
fn cl_pools_use_official_hook() {
    let r = record();
    let hook: Address = "0xC0ed888AfE87ed1AcF01D9f19bB850159e3ce434".parse().unwrap();
    let pools = r["cl"]["pools"].as_array().unwrap();
    assert!(!pools.is_empty(), "в записи нет пулов CL");
    for p in pools {
        assert_eq!(addr(w(&p["getPoolSwapHook"], 0).unwrap()), hook, "крючок пула {}", p["pool"]);
        // SwapHookLib._SWAP_FEE_CEIL = 100000 (metadex-slipstream-public cf1ae46f SwapHookLib.sol L24).
        assert!(w(&p["fee"], 0).unwrap() <= U256::from(100_000u32));
    }
}
