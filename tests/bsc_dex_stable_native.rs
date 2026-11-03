//! Файл: tests/bsc_dex_stable_native.rs
//! Что делает: нативный BNB в пулах PancakeSwap Stable / Lista (решение «122 X (нативный BNB в стейблсвапах)») без сети
//! и без моков: сопоставление метки BNB_ADDRESS 0xEeee… с обёрткой WBNB (pools/stable_native.rs) — монеты пула в
//! монеты пары, отказ при «метка + обёртка» в одном пуле и при незаданной обёртке, пара для реестра фабрики (обёртка →
//! метка), decimals метки = 18 и её множитель точности 1e18 (RATES пула: PRECISION · 10^(18 − 18)), метка совпадает
//! с BNB_ADDRESS официальных исходников (константа в контракте исполнителя — та же).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools::stable_native, math::stableswap_v1, exec::executor::kind}.

use alloy_primitives::{Address, U256, address};

use arb::venues::bsc_dex::exec::executor::kind::STABLE_NATIVE;
use arb::venues::bsc_dex::math::stableswap_v1::rate_for_decimals;
use arb::venues::bsc_dex::pools::stable_native::{
    NATIVE, NATIVE_DECIMALS, native_index, pair_coins, registry_pair,
};

/// Реальные адреса BSC: WBNB, slisBNB (монета пула Lista slisBNB/BNB 0x3DcE…6131 — scratchpad/sources/lista/README.md;
/// адрес — lista-dao/moolah@eb7d73a script/deploy_slisBNBProvider.sol L12), USDT.
const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const SLISBNB: Address = address!("0xB0b84D294e0C75A6abe60171b70edEb2EFd14A1B");
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");

#[test]
fn native_label_is_official_bnb_address() {
    // PancakeStableSwapTwoPool.sol L39, StableSwapPool.sol L54; та же константа NATIVE в contracts/ArbExecutor.sol.
    assert_eq!(
        NATIVE,
        address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE")
    );
    assert_eq!(STABLE_NATIVE, NATIVE);
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/contracts/ArbExecutor.sol"
    ))
    .unwrap();
    assert!(src.contains(
        "address internal constant NATIVE = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;"
    ));
}

#[test]
fn pool_coins_map_label_to_wrapped() {
    // Lista slisBNB/BNB: coins = [slisBNB, 0xEeee…] (фабрика сортирует адреса: slisBNB < 0xEeee…).
    let coins = [SLISBNB, NATIVE];
    assert_eq!(pair_coins(&coins, Some(WBNB)).unwrap(), vec![SLISBNB, WBNB]);
    assert_eq!(native_index(&coins), Some(1));
    // Пул без метки — как есть, обёртка не нужна.
    assert_eq!(
        pair_coins(&[USDT, SLISBNB], None).unwrap(),
        vec![USDT, SLISBNB]
    );
    assert_eq!(native_index(&[USDT, SLISBNB]), None);
    // Метка без обёртки — не берётся; метка и обёртка в одном пуле — неоднозначно, не берётся.
    assert!(pair_coins(&coins, None).is_err());
    let why = pair_coins(&[WBNB, NATIVE], Some(WBNB)).unwrap_err();
    assert!(why.contains("неоднозначной"), "{why}");
}

#[test]
fn registry_pair_asks_factory_with_label() {
    assert_eq!(
        registry_pair(SLISBNB, WBNB, Some(WBNB)),
        Some((SLISBNB, NATIVE))
    );
    assert_eq!(registry_pair(WBNB, USDT, Some(WBNB)), Some((NATIVE, USDT)));
    assert_eq!(registry_pair(SLISBNB, USDT, Some(WBNB)), None);
    assert_eq!(registry_pair(SLISBNB, WBNB, None), None);
}

#[test]
fn native_decimals_give_pool_rate() {
    // initialize: coinDecimal = 18 → RATES = PRECISION · 10^(18 − 18) = 1e18 (TwoPool L130-L139, Lista L128-L137).
    assert_eq!(NATIVE_DECIMALS, 18);
    assert_eq!(
        rate_for_decimals(NATIVE_DECIMALS),
        Some(U256::from(10u64).pow(U256::from(18u64)))
    );
}
