//! Файл: tests/bsc_dex_dodo_math.rs
//! Что делает: математика свопа DODO PMM (math/swap_dodo.rs + math/dodo_math.rs, решение 80 — экзотическая математика
//! DODO/PMM) на ОФИЦИАЛЬНЫХ числовых векторах контракта DODOEX/contractV2 (HEAD 2f1bcda). Без сети, без моков — только
//! зафиксированные официальные значения из тестов контракта (их assert'ы on-chain балансов).
//!
//! Откуда векторы (решение 23): в официальных тестах DODO комиссии lpFeeRate и mtFeeRate в контекстах равны нулю
//! (DVMContext/DSPContext: lpFeeRate передаётся как 0, модель mtFeeRate не инициализируется → getFeeRate = 0), поэтому
//! asserted on-chain баланс трейдера = ЧИСТЫЙ выход PMM-математики (querySell* вычитает нулевые комиссии). Значит
//! on-chain вектор воспроизводится нашей чистой математикой бит-в-бит. Состояние PMMState строим ровно как getPMMState
//! контракта (DVMStorage/DSPStorage): seed B0/Q0/R → adjustedTarget.
//! DVM (test/DVM/trader.test.ts «buy & sell», i=100e18, K=0.1e18): getPMMState задаёт Q0=0, R=ABOVE_ONE и решает B0.
//! Трейд 1 (sellQuote 200): receiveBase = 1952621458756349837 (баланс base 10e18 → 11952621458756349837).
//! Трейд 2 (sellBase 1, резервы после трейда 1: B=8047378541243650163, Q=200e18): receiveQuote = 103734814802481693100.
//! DSP (test/DSP/trader.test.ts, i=1e18, K=0.1e18, свежий пул B=B0=Q=Q0=1000e18, R=ONE):
//! «first buy» (sellQuote 100) receiveBase = 98914196817061816111; «first sell» (sellBase 1) receiveQuote = 999899919944970392.
//! Покрытие: _SolveQuadraticFunctionForTrade (R=ONE и R>1), _SolveQuadraticFunctionForTarget (adjustedTarget),
//! _GeneralIntegrate (R>1 продажа базы), DecimalMath, SafeMath.sqrt/divCeil. Эталон сверен независимым Python-портом
//! формул контракта — совпадает бит-в-бит.
//! Кросс-чек против живого DODO-квотера на записи реального пула — следующая фаза (семейство DODO, 86b).
//! Кто вызывает: cargo test. От чего зависит: arb::venues::bsc_dex::math::swap_dodo.

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_dodo::{
    PmmState, RState, adjusted_target, sell_base_token, sell_quote_token,
};

/// 10^18 (единица фикс-точки DODO).
fn one() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}
/// n·10^18.
fn e18(n: u64) -> U256 {
    U256::from(n) * one()
}
/// Разобрать десятичную строку wei в U256.
fn w(s: &str) -> U256 {
    s.parse().unwrap()
}

/// DVM-пул как getPMMState (DVMStorage.sol): Q0=0, R=ABOVE_ONE, B0 решается adjustedTarget по резервам B,Q.
fn dvm_state(i: U256, k: U256, b: U256, q: U256) -> PmmState {
    let mut s = PmmState {
        i,
        k,
        b,
        q,
        b0: U256::ZERO,
        q0: U256::ZERO,
        r: RState::AboveOne,
    };
    adjusted_target(&mut s).unwrap();
    s
}

/// DVM «buy & sell» (test/DVM/trader.test.ts), i=100e18, K=0.1e18.
#[test]
fn dvm_official_buy_then_sell() {
    let i = e18(100);
    let k = one() / U256::from(10u8); // 0.1e18

    // Трейд 1: свежий DVM B=10e18, Q=0. sellQuote(200) → receiveBase.
    let s1 = dvm_state(i, k, e18(10), U256::ZERO);
    assert_eq!(s1.b0, e18(10), "adjustedTarget: B0 = 10e18 на свежем DVM");
    let (recv_base, r1) = sell_quote_token(&s1, e18(200)).unwrap();
    assert_eq!(recv_base, w("1952621458756349837"));
    assert_eq!(r1, RState::AboveOne);

    // Трейд 2: резервы после трейда 1 (B=10e18−recvBase, Q=200e18). sellBase(1) → receiveQuote.
    let s2 = dvm_state(i, k, w("8047378541243650163"), e18(200));
    let (recv_quote, _r2) = sell_base_token(&s2, e18(1)).unwrap();
    assert_eq!(recv_quote, w("103734814802481693100"));
}

/// DSP свежий двусторонний пул, R=ONE: i=1e18, K=0.1e18, B=B0=Q=Q0=1000e18 (test/DSP/trader.test.ts).
fn dsp_state_r1() -> PmmState {
    PmmState {
        i: one(),
        k: one() / U256::from(10u8),
        b: e18(1000),
        q: e18(1000),
        b0: e18(1000),
        q0: e18(1000),
        r: RState::One,
    }
}

/// DSP «first buy» (sellQuote 100 при R=1) → receiveBase, ветка _ROneSellQuoteToken.
#[test]
fn dsp_official_buy_at_r_one() {
    let s = dsp_state_r1();
    let (recv_base, new_r) = sell_quote_token(&s, e18(100)).unwrap();
    assert_eq!(recv_base, w("98914196817061816111"));
    assert_eq!(new_r, RState::AboveOne);
}

/// DSP «first sell» (sellBase 1 при R=1) → receiveQuote, ветка _ROneSellBaseToken.
#[test]
fn dsp_official_sell_at_r_one() {
    let s = dsp_state_r1();
    let (recv_quote, new_r) = sell_base_token(&s, e18(1)).unwrap();
    assert_eq!(recv_quote, w("999899919944970392"));
    assert_eq!(new_r, RState::BelowOne);
}
