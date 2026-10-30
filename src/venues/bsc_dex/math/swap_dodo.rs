//! Файл: venues/bsc_dex/math/swap_dodo.rs
//! Что делает: кривая DODO PMM (решение 80 — экзотическая математика DODO/PMM) — порт официальной библиотеки
//! контракта DODO V2 PMMPricing: продажа базовой/котируемой монеты в пул по состоянию кривой, разбор R-состояний
//! (ONE / ABOVE_ONE / BELOW_ONE) с угловыми случаями «возврат к R=1», подгонка цели (adjustedTarget) и средняя цена
//! (getMidPrice). Чистая математика на целых U256 (без сети): формулы интеграла и корней — в math/dodo_math.rs.
//! Это уровень цены ОДНОГО свопа в одном PMM-пуле; граф/маршрут/сплит (решение 80), чтение состояния пула, события и
//! исполнение — следующие фазы (семейство DODO, 86b/86c). Параметры i (референсная цена) и K (наклон) и резервы B/Q
//! приходят от вызывающего (с контракта пула в 86b), здесь не задаются.
//! Охват (рекомендация техлида): V2 PMM — пулы DVM / DPP / DSP, которые ценятся on-chain и расходятся с рынком.
//! Classical V1 (цена с оракула Chainlink) этой библиотекой не считается — в проект не берётся. Различие пулов (DVM
//! односторонний: Q0=0, R=ABOVE_ONE; DSP/DPP двусторонние) — в построении PMMState на чтении (86b), не в этой математике.
//! Кто вызывает: quote/pool_quote.rs (через 86b); тесты (tests/bsc_dex_dodo_math.rs).
//! От чего зависит: math/dodo_math.rs. Настройки: нет.
//!
//! Источники (официальные, решение 23): DODOEX/contractV2 (HEAD 2f1bcda): PMMPricing.sellBaseToken / sellQuoteToken /
//! _ROne*/_RAbove*/_RBelow* / adjustedTarget / getMidPrice, enum RState, struct PMMState — contracts/lib/PMMPricing.sol.

use alloy_primitives::U256;

// Примитивы DODOMath/DecimalMath (math/dodo_math.rs) — для котировки (86b) и тестов — доступны через этот модуль.
pub use super::dodo_math::{
    div_floor, general_integrate, mul_floor, one, reciprocal_floor, solve_quadratic_for_target,
    solve_quadratic_for_trade,
};

/// PMMPricing.RState: соотношение резерва к цели. ONE — у цели (B=B0, Q=Q0); ABOVE_ONE — базовой больше цели (цена
/// копит премию при продаже базы); BELOW_ONE — котируемой больше цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RState {
    One,
    AboveOne,
    BelowOne,
}

/// PMMPricing.PMMState: состояние кривой пула. i — референсная цена (quote за base, 18 знаков); K — наклон (0..=1e18);
/// B/Q — резервы; B0/Q0 — цели; R — текущее состояние. Строится на чтении пула (86b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmmState {
    pub i: U256,
    pub k: U256,
    pub b: U256,
    pub q: U256,
    pub b0: U256,
    pub q0: U256,
    pub r: RState,
}

/// PMMPricing.sellBaseToken: трейдер отдаёт payBase базовой, получает котируемую. Возвращает: (receiveQuote, новое R).
/// None — откат/переполнение внутренней математики (см. dodo_math).
pub fn sell_base_token(s: &PmmState, pay_base: U256) -> Option<(U256, RState)> {
    match s.r {
        RState::One => {
            // R=1 → уходит ниже единицы.
            Some((r_one_sell_base(s, pay_base)?, RState::BelowOne))
        }
        RState::AboveOne => {
            let back_to_one_pay_base = s.b0.checked_sub(s.b)?; // B0−B
            let back_to_one_receive_quote = s.q.checked_sub(s.q0)?; // Q−Q0
            if pay_base < back_to_one_pay_base {
                // R остаётся выше единицы.
                let mut recv = r_above_sell_base(s, pay_base)?;
                if recv > back_to_one_receive_quote {
                    // Угловой случай точности: держим spare quote ≥ 0 (как в контракте).
                    recv = back_to_one_receive_quote;
                }
                Some((recv, RState::AboveOne))
            } else if pay_base == back_to_one_pay_base {
                Some((back_to_one_receive_quote, RState::One))
            } else {
                // Переход через единицу: добираем остаток по ветке R=1.
                let extra = r_one_sell_base(s, pay_base.checked_sub(back_to_one_pay_base)?)?;
                Some((
                    back_to_one_receive_quote.checked_add(extra)?,
                    RState::BelowOne,
                ))
            }
        }
        RState::BelowOne => Some((r_below_sell_base(s, pay_base)?, RState::BelowOne)),
    }
}

/// PMMPricing.sellQuoteToken: трейдер отдаёт payQuote котируемой, получает базовую. Возвращает: (receiveBase, новое R).
/// None — откат/переполнение внутренней математики.
pub fn sell_quote_token(s: &PmmState, pay_quote: U256) -> Option<(U256, RState)> {
    match s.r {
        RState::One => Some((r_one_sell_quote(s, pay_quote)?, RState::AboveOne)),
        RState::AboveOne => Some((r_above_sell_quote(s, pay_quote)?, RState::AboveOne)),
        RState::BelowOne => {
            let back_to_one_pay_quote = s.q0.checked_sub(s.q)?; // Q0−Q
            let back_to_one_receive_base = s.b.checked_sub(s.b0)?; // B−B0
            if pay_quote < back_to_one_pay_quote {
                let mut recv = r_below_sell_quote(s, pay_quote)?;
                if recv > back_to_one_receive_base {
                    recv = back_to_one_receive_base;
                }
                Some((recv, RState::BelowOne))
            } else if pay_quote == back_to_one_pay_quote {
                Some((back_to_one_receive_base, RState::One))
            } else {
                let extra = r_one_sell_quote(s, pay_quote.checked_sub(back_to_one_pay_quote)?)?;
                Some((
                    back_to_one_receive_base.checked_add(extra)?,
                    RState::AboveOne,
                ))
            }
        }
    }
}

// ============ R = 1 ============

/// _ROneSellBaseToken: Q2 через _SolveQuadraticFunctionForTrade(Q0, Q0, payBase, i, K). None — откат.
fn r_one_sell_base(s: &PmmState, pay_base: U256) -> Option<U256> {
    solve_quadratic_for_trade(s.q0, s.q0, pay_base, s.i, s.k)
}

/// _ROneSellQuoteToken: _SolveQuadraticFunctionForTrade(B0, B0, payQuote, 1/i, K). None — откат.
fn r_one_sell_quote(s: &PmmState, pay_quote: U256) -> Option<U256> {
    solve_quadratic_for_trade(s.b0, s.b0, pay_quote, reciprocal_floor(s.i)?, s.k)
}

// ============ R < 1 ============

/// _RBelowSellQuoteToken: _GeneralIntegrate(Q0, Q+payQuote, Q, 1/i, K). None — откат.
fn r_below_sell_quote(s: &PmmState, pay_quote: U256) -> Option<U256> {
    general_integrate(
        s.q0,
        s.q.checked_add(pay_quote)?,
        s.q,
        reciprocal_floor(s.i)?,
        s.k,
    )
}

/// _RBelowSellBaseToken: _SolveQuadraticFunctionForTrade(Q0, Q, payBase, i, K). None — откат.
fn r_below_sell_base(s: &PmmState, pay_base: U256) -> Option<U256> {
    solve_quadratic_for_trade(s.q0, s.q, pay_base, s.i, s.k)
}

// ============ R > 1 ============

/// _RAboveSellBaseToken: _GeneralIntegrate(B0, B+payBase, B, i, K). None — откат.
fn r_above_sell_base(s: &PmmState, pay_base: U256) -> Option<U256> {
    general_integrate(s.b0, s.b.checked_add(pay_base)?, s.b, s.i, s.k)
}

/// _RAboveSellQuoteToken: _SolveQuadraticFunctionForTrade(B0, B, payQuote, 1/i, K). None — откат.
fn r_above_sell_quote(s: &PmmState, pay_quote: U256) -> Option<U256> {
    solve_quadratic_for_trade(s.b0, s.b, pay_quote, reciprocal_floor(s.i)?, s.k)
}

// ============ Вспомогательные ============

/// PMMPricing.adjustedTarget: пересчитать цель по состоянию. BELOW_ONE → Q0 = solveTarget(Q, B−B0, i, K);
/// ABOVE_ONE → B0 = solveTarget(B, Q−Q0, 1/i, K); ONE — без изменений. Меняет state.b0/q0 на месте. None — откат.
pub fn adjusted_target(s: &mut PmmState) -> Option<()> {
    match s.r {
        RState::BelowOne => {
            s.q0 = solve_quadratic_for_target(s.q, s.b.checked_sub(s.b0)?, s.i, s.k)?;
        }
        RState::AboveOne => {
            s.b0 = solve_quadratic_for_target(
                s.b,
                s.q.checked_sub(s.q0)?,
                reciprocal_floor(s.i)?,
                s.k,
            )?;
        }
        RState::One => {}
    }
    Some(())
}

/// PMMPricing.getMidPrice: средняя цена кривой при текущем состоянии (quote за base, 18 знаков). None — откат.
pub fn get_mid_price(s: &PmmState) -> Option<U256> {
    if s.r == RState::BelowOne {
        let r = div_floor(s.q0.checked_mul(s.q0)? / nz(s.q)?, s.q)?;
        let r = one().checked_sub(s.k)?.checked_add(mul_floor(s.k, r)?)?;
        div_floor(s.i, r)
    } else {
        let r = div_floor(s.b0.checked_mul(s.b0)? / nz(s.b)?, s.b)?;
        let r = one().checked_sub(s.k)?.checked_add(mul_floor(s.k, r)?)?;
        mul_floor(s.i, r)
    }
}

/// None — x = 0 (в контракте этот div откатился бы). Ошибок иначе нет.
fn nz(x: U256) -> Option<U256> {
    if x.is_zero() { None } else { Some(x) }
}
