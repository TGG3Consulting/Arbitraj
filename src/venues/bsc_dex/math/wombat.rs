//! Файл: venues/bsc_dex/math/wombat.rs
//! Что делает: решение 484 — выход обмена пула Wombat Exchange на точный вход один-в-один с развёрнутыми реализациями
//! Arbitrum (HighCovRatioFeePoolV2 / DynamicPoolV2 поверх PoolV2 + CoreV2):
//! - `quote_from` — PoolV2._quoteFrom (множитель _quoteFactor динамического пула, идеальный выход CoreV2, проверка cash
//!   выхода, haircut) и надстройка HighCovRatioFeePoolV2._quoteFrom (комиссия за покрытие входа выше startCovRatio,
//!   откат выше endCovRatio);
//! - `potential` — PoolV2.quotePotentialSwap (вход в единицах монеты → toWad → quote_from → fromWad);
//! - `swap_out` — PoolV2.swap: пауза пула и актива входа, toWad, quote_from, addCash (uint120), removeCash (cash ≥ выход
//!   + haircut), покрытие выхода после обмена ≥ 1 % (иначе WOMBAT_FORBIDDEN), fromWad. Только точный вход.
//!
//! Кто вызывает: quote/pool_quote.rs (ветка StableSwap со StableData.wombat), quote/wombat_depth.rs.
//! От чего зависит: math/wombat_core.rs, core/types (StableData, WombatData).
//! Настройки: нет.
//!
//! Источник: wombat-exchange/v1-core 2ac067b8 contracts/wombat-core/pool/PoolV2.sol L745-L804 (_quoteFactor,
//! _quoteFrom), L809-L831 (_swap), L843-L876 (swap), L888-L907 (quotePotentialSwap); HighCovRatioFeePoolV2.sol
//! L43-L62 (_highCovRatioFee), L67-L110 (_quoteFrom); DynamicPoolV2.sol L27-L34 (_quoteFactor); asset/Asset.sol L144-L157
//! (addCash, removeCash). Сверка с развёрнутыми реализациями — docs/sources/wombat_arbitrum/README.md (путь обмена тот
//! же; отличия PoolV2 — только _mintFee / mintFee и проверка актива в pauseAsset).

use alloy_primitives::{I256, U256};

use crate::core::types::{StableData, WombatData};

use super::wombat_core::{from_wad, swap_quote, to_wad, wad, wdiv, wmul};

/// Предел uint120 (Asset.sol: cash и liability — uint120; addCash: `amount > type(uint120).max` — откат, `cash +=` —
/// проверка 0.8). Ошибок нет.
fn u120_max() -> U256 {
    (U256::from(1u8) << 120) - U256::from(1u8)
}

/// int256(uint256 x) — у значений uint120 / uint128 / малых произведений без потери. None — x ≥ 2^255.
fn int(x: U256) -> Option<I256> {
    I256::try_from(x).ok()
}

/// Множитель _quoteFactor (DynamicPoolV2.sol L27-L34): (1e18 · цена входа) / цена выхода; у не динамического — 1e18.
/// None — цены выхода ноль, переполнение.
fn factor(w: &WombatData, i: usize, j: usize) -> Option<U256> {
    if w.price.is_empty() {
        return Some(wad());
    }
    wad()
        .checked_mul(*w.price.get(i)?)?
        .checked_div(*w.price.get(j)?)
}

/// HighCovRatioFeePoolV2._highCovRatioFee (L43-L62): доля комиссии за покрытие. None — final > endCovRatio
/// (WOMBAT_COV_RATIO_LIMIT_EXCEEDED), деление на ноль. Блок unchecked контракта — wrapping.
fn high_cov_fee(w: &WombatData, init: U256, fin: U256) -> Option<U256> {
    let (start, end) = (w.start_cov, w.end_cov);
    if fin > end {
        return None;
    }
    if fin <= start || fin <= init {
        return Some(U256::ZERO);
    }
    let a = if init <= start {
        U256::ZERO
    } else {
        let d = init.wrapping_sub(start);
        d.wrapping_mul(d)
    };
    let d = fin.wrapping_sub(start);
    let b = d.wrapping_mul(d);
    let q = b.wrapping_sub(a).checked_div(fin.wrapping_sub(init))? / U256::from(2u8);
    wdiv(q, end.wrapping_sub(start))
}

/// Идеальный выход PoolV2._quoteFrom до haircut (PoolV2.sol L773-L793): множитель _quoteFactor к стороне входа,
/// CoreV2._swapQuoteFunc, проверка cash выхода. Принимает: состояние Wombat, усиление A (ampFactor), индексы монет
/// входа i и выхода j, вход в 18 знаках. Возвращает: выход в 18 знаках. None — откат (CORE_UNDERFLOW,
/// CASH_NOT_ENOUGH, деление на ноль, переполнение).
pub fn ideal(w: &WombatData, amp: U256, i: usize, j: usize, amount: U256) -> Option<U256> {
    let (cash_in, liab_in) = (*w.cash.get(i)?, *w.liability.get(i)?);
    let (cash_out, liab_out) = (*w.cash.get(j)?, *w.liability.get(j)?);
    let scale = factor(w, i, j)?;
    let (mut fc, mut fl, mut fa) = (cash_in, liab_in, int(amount)?);
    if scale != wad() {
        fc = fc.checked_mul(scale)? / wad();
        fl = fl.checked_mul(scale)? / wad();
        fa = fa.checked_mul(int(scale)?)?.checked_div(int(wad())?)?;
    }
    let out = swap_quote(int(fc)?, int(cash_out)?, int(fl)?, int(liab_out)?, fa, int(amp)?)?;
    // PoolV2.sol L791-L793: выход больше cash монеты выхода — WOMBAT_CASH_NOT_ENOUGH.
    (cash_out >= out).then_some(out)
}

/// _quoteFrom на точный вход (PoolV2.sol L761-L804 + HighCovRatioFeePoolV2.sol L67-L91, ветка fromAmount ≥ 0).
/// Принимает: как ideal. Возвращает: (выход, haircut) в 18 знаках монеты выхода. None — откат контракта (как ideal и
/// COV_RATIO_LIMIT_EXCEEDED).
pub fn quote_from(
    w: &WombatData,
    amp: U256,
    i: usize,
    j: usize,
    amount: U256,
) -> Option<(U256, U256)> {
    let (cash_in, liab_in) = (*w.cash.get(i)?, *w.liability.get(i)?);
    let ideal = ideal(w, amp, i, j, amount)?;
    let mut haircut = wmul(ideal, w.haircut)?;
    let mut actual = ideal.checked_sub(haircut)?;
    // HighCovRatioFeePoolV2.sol L74-L92: покрытие входа после обмена (без множителя — исходные cash / liability).
    let fin = wdiv(cash_in.checked_add(amount)?, liab_in)?;
    if fin > w.start_cov {
        let fee = wmul(high_cov_fee(w, wdiv(cash_in, liab_in)?, fin)?, actual)?;
        actual = actual.checked_sub(fee)?;
        haircut = haircut.wrapping_add(fee);
    }
    Some((actual, haircut))
}

/// PoolV2.quotePotentialSwap (L888-L907) на положительный вход. Принимает: состояние StableSwap пула Wombat, индексы,
/// вход в единицах монеты. Возвращает: выход в единицах монеты выхода. None — нет части Wombat, вход 0
/// (WOMBAT_ZERO_AMOUNT), откат _quoteFrom.
pub fn potential(d: &StableData, i: usize, j: usize, amount: U256) -> Option<U256> {
    let w = d.wombat.as_deref()?;
    if amount.is_zero() || i == j {
        return None;
    }
    let wad_in = to_wad(amount, *w.decimals.get(i)?)?;
    let (out, _) = quote_from(w, d.amp, i, j, wad_in)?;
    from_wad(out, *w.decimals.get(j)?)
}

/// PoolV2.swap (L843-L876) → _swap (L809-L831): выход обмена на точный вход. Принимает: как potential. Возвращает:
/// выход в единицах монеты выхода. None — откат обмена: пул на паузе (whenNotPaused), актив входа на паузе
/// (requireAssetNotPaused(fromToken)), вход 0, откат _quoteFrom, addCash сверх uint120, removeCash больше cash,
/// покрытие выхода после обмена < 1 % (WOMBAT_FORBIDDEN).
pub fn swap_out(d: &StableData, i: usize, j: usize, amount: U256) -> Option<U256> {
    let w = d.wombat.as_deref()?;
    if d.halted || *w.asset_paused.get(i)? || amount.is_zero() || i == j {
        return None;
    }
    let wad_in = to_wad(amount, *w.decimals.get(i)?)?;
    let (out, haircut) = quote_from(w, d.amp, i, j, wad_in)?;
    // Asset.addCash (L144-L147): вход > uint120 — ASSET_OVERFLOW, сумма > uint120 — откат 0.8.
    if wad_in > u120_max() || w.cash.get(i)?.checked_add(wad_in)? > u120_max() {
        return None;
    }
    // Asset.removeCash (L154-L157): `require(cash >= amount)`; сумма — в unchecked (PoolV2.sol L824-L826).
    let cash_out = *w.cash.get(j)?;
    let take = out.wrapping_add(haircut);
    if cash_out < take {
        return None;
    }
    // PoolV2.sol L830: покрытие выхода после обмена < 1 % — WOMBAT_FORBIDDEN.
    if wdiv(cash_out - take, *w.liability.get(j)?)? < wad() / U256::from(100u8) {
        return None;
    }
    from_wad(out, *w.decimals.get(j)?)
}
