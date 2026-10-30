//! Файл: tests/bsc_dex_woofi_math.rs
//! Что делает: математика свопа WOOFi Swap V2 sPMM (math/swap_woofi.rs, решение 80 — экзотическая математика WooFi) на
//! ОФИЦИАЛЬНОМ сценарии контракта woo-finance/WooPoolV2 (HEAD d355ca863bbc0db967a58778ea17c6cc9a1675dd). Без сети, без
//! моков — только зафиксированные официальные входы и официальные проверки.
//!
//! Откуда «векторы» (решение 23): официальные тесты WooPP (test/typescript/WooPPv2.test.ts, блоки «wooPP query» и
//! «wooPP swap») НЕ ассертят точные целые выходы — они проверяют СВОЙСТВА на точных официальных входах: нетто-выход
//! строго меньше бенчмарка `price·amount·(1 − fee)` и проскальзывание (benchmark − net)/benchmark ниже официального
//! порога (0,0012 для большинства, 0,0013 для 10 BTC). Мы воспроизводим эти официальные проверки бит-в-бит на нашей
//! математике. Дополнительно фиксируем ТОЧНЫЕ целые выходы как регрессионные якоря — они сверены независимым
//! Python-портом формул контракта (как сделано для DODO 86a): совпадают бит-в-бит.
//!
//! Официальные входы (WooPPv2.test.ts beforeEach): price = 20000·1e8 = 2e12 (BTC_PRICE·PRICE_DEC), spread = 0,001e18,
//! coeff = 0,000000001e18 = 1e9, feeRate(btc) = 100 («1 в 1e5» = 0,1%); токены TestERC20Token (18 знаков) →
//! baseDec = quoteDec = 1e18; оракул без Chainlink-ссылки → decimals = 8 → priceDec = 1e8.
//! Кейсы sellBase: 1, 3, 10 BTC (query accuracy1/1_2/1_3, sellBase accuracy1/2). Кейсы sellQuote: 10000, 100000 USDT
//! (query accuracy2_1/2_2, sellQuote accuracy1/2). WOO (base→base, price 0,15·1e8) — проверка calc для второй ноги.
//! Покрыто: обе ценовые функции, комиссия на входе/выходе, newPrice, граничный случай (coef ниже нуля → None).
//! Кросс-чек против живого WooFi-квотера (WooRouterV2.querySwap) на реальном пуле — фаза B.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::bsc_dex::math::swap_woofi.

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_woofi::{
    Decs, WooState, calc_base_amount_sell_quote, calc_quote_amount_sell_base, sell_base, sell_quote,
};

/// 10^n.
fn p10(n: u32) -> U256 {
    U256::from(10u8).pow(U256::from(n))
}
/// Разобрать десятичную строку wei.
fn w(s: &str) -> U256 {
    s.parse().unwrap()
}
/// Приблизить U256-wei (18 знаков) к f64 для официальной проверки проскальзывания. Делим на 1e9 в U256 (результат
/// ≤ ~2e14, влезает в u64), затем ещё на 1e9 во float — итог в единицах токена с 9 знаками дроби.
fn to_f64(x: U256) -> f64 {
    let scaled = u64::try_from(x / p10(9)).unwrap();
    scaled as f64 / 1e9
}

/// Официальный сценарий BTC (WooPPv2.test.ts): price=2e12, spread=1e15, coeff=1e9, feeRate=100, decimals 18/18/8.
fn btc() -> (WooState, Decs, U256) {
    let s = WooState {
        price: U256::from(20000u32) * p10(8), // 2e12
        spread: p10(15),                      // 0.001e18
        coeff: p10(9),                        // 0.000000001e18
    };
    let d = Decs {
        price_dec: p10(8),
        quote_dec: p10(18),
        base_dec: p10(18),
    };
    (s, d, U256::from(100u8)) // feeRate
}

/// sellBase (btc→usdt): официальные проверки query/sellBase accuracy1/1_2/1_3/accuracy2 + точные якоря.
#[test]
fn sell_base_official_scenario() {
    let (s, d, fee) = btc();
    let one = p10(18);

    // Точные целые якоря (независимый Python-порт формул контракта).
    let net1 = sell_base(one, fee, &s, &d).unwrap();
    assert_eq!(net1, w("19959620400000000000000"));
    let net3 = sell_base(U256::from(3u8) * one, fee, &s, &d).unwrap();
    assert_eq!(net3, w("59876463600000000000000"));
    let net10 = sell_base(U256::from(10u8) * one, fee, &s, &d).unwrap();
    assert_eq!(net10, w("199560240000000000000000"));

    // Официальные проверки WooPPv2.test.ts: net < benchmark и slippage < порог.
    for (num, cap) in [(1u64, 0.0012), (3, 0.0012), (10, 0.0013)] {
        let net = sell_base(U256::from(num) * one, fee, &s, &d).unwrap();
        let benchmark = 20000.0 * (num as f64) * (1.0 - 0.001);
        let net_f = to_f64(net);
        assert!(
            net_f < benchmark,
            "n={num}: net {net_f} < benchmark {benchmark}"
        );
        let slip = (benchmark - net_f) / benchmark;
        assert!(slip < cap, "n={num}: slippage {slip} < {cap}");
    }

    // Валовый выход (до комиссии) и newPrice — точные якоря (gross для 1 BTC = 19979.6 USDT).
    let (gross1, new_price1) = calc_quote_amount_sell_base(one, &s, &d).unwrap();
    assert_eq!(gross1, w("19979600000000000000000"));
    assert_eq!(new_price1, w("1999920000000")); // price·(1 − 2·c·price·1) < 2e12
}

/// sellQuote (usdt→btc): официальные проверки query/sellQuote accuracy2_1/2_2/accuracy1/2 + точные якоря.
#[test]
fn sell_quote_official_scenario() {
    let (s, d, fee) = btc();
    let one = p10(18);

    // Точные целые якоря.
    let base10k = sell_quote(U256::from(10000u32) * one, fee, &s, &d).unwrap();
    assert_eq!(base10k, w("498995509995000000"));
    let base100k = sell_quote(U256::from(100000u32) * one, fee, &s, &d).unwrap();
    assert_eq!(base100k, w("4989505999500000000"));

    // Официальные проверки: base < benchmark и slippage < 0.0012.
    for u in [10000u64, 100000] {
        let base = sell_quote(U256::from(u) * one, fee, &s, &d).unwrap();
        let benchmark = (u as f64 / 20000.0) * (1.0 - 0.001);
        let base_f = to_f64(base);
        assert!(
            base_f < benchmark,
            "u={u}: base {base_f} < benchmark {benchmark}"
        );
        let slip = (benchmark - base_f) / benchmark;
        assert!(slip < 0.0012, "u={u}: slippage {slip} < 0.0012");
    }

    // newPrice растёт при покупке base (sellQuote): price·(1 + 2·c·quote) > 2e12. Комиссия снимается с входа.
    let swap_fee = (U256::from(10000u32) * one) * fee / U256::from(100000u32);
    let net_quote = U256::from(10000u32) * one - swap_fee;
    let (_, new_price) = calc_base_amount_sell_quote(net_quote, &s, &d).unwrap();
    assert_eq!(new_price, w("2000039960000")); // > 2e12
}

/// WOO как base (price 0,15·1e8) — вторая нога base→base (swapBaseToBase): calc_base_amount_sell_quote на цене 0.15.
/// Проверка якоря из независимого порта: 1 USDT-эквивалент котируемой → base по цене 0.15 (без спреда/коэфф влияние).
#[test]
fn woo_price_calc_anchor() {
    let s = WooState {
        price: U256::from(15u8) * p10(6), // 0.15·1e8 = 15e6
        spread: p10(15),
        coeff: p10(9),
    };
    let d = Decs {
        price_dec: p10(8),
        quote_dec: p10(18),
        base_dec: p10(18),
    };
    let (base, _) = calc_base_amount_sell_quote(p10(18), &s, &d).unwrap();
    assert_eq!(base, w("6659999993333333333"));
}

/// Граничный случай по официальной формуле: сумма столь велика, что coef уходит ниже нуля (1e18 − drop − spread < 0)
/// — контракт откатился бы на вычитании, у нас `None`. Для sellBase coef < 0 при base·c·price/decs > ~1e18.
#[test]
fn coef_underflow_is_none() {
    let (s, d, _) = btc();
    // base = 1e27: drop = c·base·price/baseDec/priceDec = 1e9·1e27·2e12/1e18/1e8 = 2e22 > 1e18 → coef < 0.
    let huge = p10(27);
    assert!(calc_quote_amount_sell_base(huge, &s, &d).is_none());
    assert!(sell_base(huge, U256::from(100u8), &s, &d).is_none());

    // sellQuote: quote·coeff/quoteDec > 1e18 → coef < 0. quote = 2e27: 2e27·1e9/1e18 = 2e18 > 1e18.
    let huge_q = U256::from(2u8) * p10(27);
    assert!(calc_base_amount_sell_quote(huge_q, &s, &d).is_none());
}

/// price = 0 → деление на price откатилось бы → None (woFeasible отсеял бы это раньше, но математика защищена).
#[test]
fn zero_price_is_none() {
    let d = Decs {
        price_dec: p10(8),
        quote_dec: p10(18),
        base_dec: p10(18),
    };
    let s = WooState {
        price: U256::ZERO,
        spread: p10(15),
        coeff: p10(9),
    };
    assert!(calc_base_amount_sell_quote(p10(18), &s, &d).is_none());
}
