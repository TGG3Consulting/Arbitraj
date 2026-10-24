//! Файл: tests/bsc_dex_native.rs
//! Что делает: нативная монета BNB (решение 62) без сети:
//! 1) план сделки: продажа BNB — команды Universal Router [WRAP_ETH, обмен (платит роутер), BALANCE_CHECK_ERC20], value
//!    транзакции = сумма; покупка — [обмен (выход роутеру), UNWRAP_WETH(кошелёк, минимум)], без BALANCE_CHECK и value;
//!    входы WRAP/UNWRAP — abi.encode(recipient, amount) (Dispatcher.sol обоих UR, exec/ur.rs); то же для V3 через UR
//!    PancakeSwap и Uniswap (путь tokenIn | fee | tokenOut);
//! 2) пул V4 / Infinity на стороне BNB (один или через промежуточную) — честный отказ NoRoute (решение 62);
//! 3) списки ТОЧНЫХ вызовов узла для записи (tests/data/native_calls_2026-10-24.json и native_calls_2_2026-10-25.json)
//!    собраны этим же кодом — тест сверяет файлы с кодом. Перезаписать: cargo test --test bsc_dex_native -- --ignored
//!
//! Живые ответы узла — tests/bsc_dex_native_live.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common (окружение, план, вызовы), arb::venues::bsc_dex::exec::{native, chain, ur}.

mod native_common;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

use arb::core::types::Side;
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::exec::chain::Step;
use arb::venues::bsc_dex::exec::native;
use arb::venues::bsc_dex::exec::plan::Order;
use arb::venues::bsc_dex::exec::ur::{self, ADDRESS_THIS};
use arb::venues::bsc_dex::pools::registry::Entry;
use native_common::*;

#[tokio::test]
async fn sell_wraps_and_buy_unwraps() {
    let c = ctx("plan").await;
    let wallet = HOLDER;
    let amount = U256::from(10u64).pow(U256::from(16u8));
    let sell = native_plan(
        &c,
        &entry("pancakeswap_v2", None),
        Side::Sell,
        amount,
        wallet,
    )
    .await;
    assert!(
        sell.approvals.is_empty(),
        "продажа BNB: платит роутер, разрешений нет"
    );
    assert_eq!(sell.value(), amount);
    let (cmds, inputs) = decode(&sell.data(U256::from(5u8)));
    assert_eq!(
        cmds,
        vec![ur::WRAP_ETH, ur::V2_SWAP_EXACT_IN, ur::BALANCE_CHECK_ERC20]
    );
    assert_eq!(
        inputs[0],
        Bytes::from((ADDRESS_THIS, amount).abi_encode_params())
    );
    // V2: (recipient, amountIn, amountOutMin, path, payerIsUser = false).
    let (to, a_in, _, path, payer): (Address, U256, U256, Vec<Address>, bool) =
        SolValue::abi_decode_params(&inputs[1]).unwrap();
    assert_eq!(
        (to, a_in, path, payer),
        (wallet, amount, vec![WBNB, USDT], false)
    );
    assert_eq!(inputs[2], ur::balance_check(wallet, USDT, U256::from(6u8)));

    let usdt = U256::from(5u64) * U256::from(10u64).pow(U256::from(18u8));
    let buy = native_plan(&c, &entry("pancakeswap_v2", None), Side::Buy, usdt, wallet).await;
    assert_eq!(buy.value(), U256::ZERO);
    let (cmds, inputs) = decode(&buy.data(U256::ZERO));
    assert_eq!(cmds, vec![ur::V2_SWAP_EXACT_IN, ur::UNWRAP_WETH]);
    let (to, _, _, path, _): (Address, U256, U256, Vec<Address>, bool) =
        SolValue::abi_decode_params(&inputs[0]).unwrap();
    assert_eq!((to, path), (ADDRESS_THIS, vec![USDT, WBNB]));
    assert_eq!(
        inputs[1],
        Bytes::from((wallet, U256::from(1u8)).abi_encode_params())
    );
}

#[tokio::test]
async fn v4_and_infinity_on_native_side_are_refused() {
    let c = ctx("v4").await;
    let o = |a, b| Order {
        token_in: a,
        token_out: b,
        amount_in: U256::from(1u8),
        min_out: U256::from(1u8),
    };
    let step = |e: Entry, ord, fw, tw| Step {
        entry: e,
        order: ord,
        from_wallet: fw,
        to_wallet: tw,
    };
    let mut legs = vec![vec![
        step(
            entry("pancakeswap_infinity_cl", None),
            o(WBNB, USDT),
            true,
            false,
        ),
        step(entry("pancakeswap_v2", None), o(USDT, WBNB), false, true),
    ]];
    let err = native::adapt(&c, Side::Sell, &mut legs).unwrap_err();
    assert_eq!(err.code, ErrorCode::NoRoute);
    // Один пул V4 / Infinity на стороне BNB — тоже отказ (решение 62: живой проверки нет), в обе стороны.
    for kind in [
        "pancakeswap_infinity_cl",
        "pancakeswap_infinity_bin",
        "uniswap_v4",
    ] {
        for side in [Side::Sell, Side::Buy] {
            let (tin, tout) = if side == Side::Sell {
                (WBNB, USDT)
            } else {
                (USDT, WBNB)
            };
            let mut legs = vec![vec![step(entry(kind, None), o(tin, tout), true, true)]];
            let err = native::adapt(&c, side, &mut legs).unwrap_err();
            assert_eq!(err.code, ErrorCode::NoRoute, "{kind} {side:?}");
        }
        assert!(!native::pool_ok(entry(kind, None).pool.kind));
    }
}

/// V3: продажа и покупка BNB через UR PancakeSwap и Uniswap — те же команды, путь V3 с комиссией 500.
#[tokio::test]
async fn v3_through_both_routers() {
    let c = ctx("v3").await;
    let e16 = U256::from(10u64).pow(U256::from(16u8));
    for kind in ["pancakeswap_v3", "uniswap_v3"] {
        let e = entry(kind, Some(500));
        let sell = native_plan(&c, &e, Side::Sell, e16, HOLDER).await;
        assert_eq!(sell.value(), e16);
        let (cmds, inputs) = decode(&sell.data(U256::ZERO));
        assert_eq!(
            cmds,
            vec![ur::WRAP_ETH, ur::V3_SWAP_EXACT_IN, ur::BALANCE_CHECK_ERC20]
        );
        let (to, a_in, _, path, payer): (Address, U256, U256, Bytes, bool) =
            SolValue::abi_decode_params(&inputs[1]).unwrap();
        let mut want = WBNB.to_vec();
        want.extend_from_slice(&[0, 0x01, 0xf4]);
        want.extend_from_slice(USDT.as_slice());
        assert_eq!(
            (to, a_in, path.to_vec(), payer),
            (HOLDER, e16, want, false),
            "{kind}"
        );
        let buy = native_plan(&c, &e, Side::Buy, e16, HOLDER).await;
        let (cmds, _) = decode(&buy.data(U256::ZERO));
        assert_eq!(cmds, vec![ur::V3_SWAP_EXACT_IN, ur::UNWRAP_WETH], "{kind}");
    }
}

#[tokio::test]
async fn native_calls_files_match_code() {
    for (file, want) in [(CALLS, calls().await), (CALLS2, calls2::calls2().await)] {
        let have: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(
            have, want,
            "{file} устарел: cargo test --test bsc_dex_native -- --ignored"
        );
    }
}

#[tokio::test]
#[ignore = "вручную: пишет списки вызовов для записи ответов узла"]
async fn write_native_calls() {
    for (file, v) in [(CALLS, calls().await), (CALLS2, calls2::calls2().await)] {
        std::fs::write(file, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
    }
}

/// Путь BNB через промежуточную: шаг со стороны BNB в пуле Infinity / V4 отфильтрован (решение 62) — путь через эту
/// промежуточную не собирается, с причиной, и котировка берёт прямой маршрут (quote/direct_route.rs).
#[tokio::test]
async fn via_skips_infinity_on_native_side() {
    use arb::core::types::{Asset, CheckState, CoinCheck, PoolRef, TokenTaxes, VenueId};
    use arb::venues::bsc_dex::parts::Parts;
    use arb::venues::bsc_dex::pools::mids::Mid;
    use arb::venues::bsc_dex::quote::via::Via;
    // Промежуточная — CAKE (адрес как у BingX в сети BSC, записи tests/data/indexers_2026-10-24).
    let cake = alloy_primitives::address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
    let p = Parts::new(ctx("via").await);
    let asset = |t: &str, a| Asset {
        ticker: t.into(),
        bsc_contract: Some(a),
    };
    p.tokens
        .set(&[asset("BNB", WBNB), asset("USDT", USDT), asset("CAKE", cake)]);
    let zero = TokenTaxes {
        buy_pct: 0.into(),
        sell_pct: 0.into(),
        transfer_pct: 0.into(),
    };
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: asset("CAKE", cake),
        taxes: CheckState::Known(zero.clone()),
        source: "тест".into(),
        checked_at: None,
    });
    // P1 «CAKE — USDT» (V2), P2 «BNB — CAKE» (Infinity CL) — у P2 сторона BNB.
    let mut p1 = entry("pancakeswap_v2", None);
    (p1.coin, p1.usdt, p1.ticker) = (cake, USDT, "CAKE".into());
    p1.pool.reference = PoolRef::Contract(cake);
    let mut p2 = entry("pancakeswap_infinity_cl", None);
    (p2.coin, p2.usdt) = (WBNB, cake);
    p2.pool.reference = PoolRef::Contract(WBNB);
    p.registry.put(p1);
    p.registry.put(p2);
    let q = p.quoting();
    let mid = Mid {
        address: cake,
        ticker: "CAKE".into(),
    };
    for side in [Side::Sell, Side::Buy] {
        let v = Via {
            q: &q,
            coin: WBNB,
            usdt: USDT,
            side,
            taxes: &zero,
            coin_dec: 18,
            usdt_dec: 18,
        };
        assert!(v.native());
        let why = v.pools(&mid, &|_| true).unwrap_err();
        assert!(why.contains("не поддержан"), "{why}");
    }
}
