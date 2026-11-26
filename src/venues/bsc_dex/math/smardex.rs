//! Файл: venues/bsc_dex/math/smardex.rs
//! Что делает: выход обмена пары SmarDex на точный вход (решение 288) — порт SmardexLibrary.getAmountOut и
//! getUpdatedPriceAverage с теми же округлениями и откатами (Solidity 0.8: переполнение и вычитание ниже нуля — откат →
//! None). Цена пары зависит от ВРЕМЕНИ блока: средняя цена (priceAverage) пересчитывается к block.timestamp обмена, от
//! неё зависит, делится ли обмен на два шага и пересчитываются ли фиктивные резервы.
//! Кто вызывает: math/smardex_swap.rs (swap_out — средняя цена к времени блока и проверки пары), quote/pool_quote.rs
//! (Params стороны); тесты на записанных ответах цепи.
//! От чего зависит: alloy-primitives.
//! Настройки: нет (состояние пары и время блока — pools/load_smardex.rs).
//!
//! Источник (решение 23): исходник фабрики BSC 0xA8EF6FEa013034E62E2C4A9Ec1CDb059fE23Af33 — Sourcify match и
//! официальный репозиторий github.com/SmarDex-Ecosystem/dex-smart-contracts@74d0444 deployments/bsc/SmardexFactory.json
//! (solcInputs/1a3e5ed825e3a0bcaf935206aae1ecd3.json; тексты совпали с Sourcify): contracts/core/libraries/
//! SmardexLibrary.sol — константы L12-L24, approxEq L45-L51, ratioApproxEq L61-L63, getUpdatedPriceAverage L76-L109,
//! computeFirstTradeQtyIn L120-L140, computeFictiveReserves L187-L209, applyKConstRuleOut L220-L245, getAmountOut
//! L292-L406; contracts/core/SmardexPair.sol swap L155-L319 (средняя цена к block.timestamp L202-L214, резерв =
//! баланс − feeToAmount L184-L196, _update: фиктивные резервы не 0 и не больше uint128 L358-L359). Выдержки —
//! tests/data/smardex_2026-11-15/source_excerpt.txt.

use alloy_primitives::U256;

/// FEES_BASE = 1 000 000 (L12) — основание комиссий пары, свойство протокола.
pub const FEES_BASE: u64 = 1_000_000;
/// APPROX_PRECISION / APPROX_PRECISION_BASE = 1 / 1 000 000 (L18, L21).
const APPROX_BASE: u64 = 1_000_000;

/// Состояние пары для расчёта (стороны уже развёрнуты по направлению обмена).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    pub reserve_in: U256,
    pub reserve_out: U256,
    pub fictive_in: U256,
    pub fictive_out: U256,
    pub price_in: U256,
    pub price_out: U256,
    pub fees_lp: U256,
    pub fees_pool: U256,
}

/// Число u64 как U256 (константы протокола в формулах). Принимает: число. Возвращает: U256. Ошибок нет.
fn u(x: u64) -> U256 {
    U256::from(x)
}

/// approxEq (L45-L51). Ошибок нет; None — переполнение.
fn approx_eq(x: U256, y: U256, exact_match: bool) -> Option<bool> {
    if exact_match && x == y {
        return Some(true);
    }
    Some(if x > y {
        x < y.checked_add(y.checked_mul(U256::from(1u8))? / u(APPROX_BASE))?
    } else {
        y < x.checked_add(x.checked_mul(U256::from(1u8))? / u(APPROX_BASE))?
    })
}

/// computeFirstTradeQtyIn (L120-L140): какая часть входа (уже без комиссий) идёт первым шагом. None — переполнение.
fn first_in(p: &Params, amount: U256) -> Option<U256> {
    let base = u(FEES_BASE);
    if p.fictive_out.checked_mul(p.price_in)? <= p.fictive_in.checked_mul(p.price_out)? {
        return Some(amount);
    }
    let to_sub = p.fictive_in.checked_mul(
        (base * u(2))
            .checked_sub(p.fees_pool * u(2))?
            .checked_sub(p.fees_lp)?,
    )?;
    let to_div = base.checked_sub(p.fees_pool)?.checked_mul(u(2))?;
    let fee_mul = base
        .checked_sub(p.fees_pool)?
        .checked_sub(p.fees_lp)?
        .checked_mul(base.checked_sub(p.fees_pool)?)?;
    let in_sqrt = p
        .fictive_in
        .checked_mul(p.fictive_out)?
        .checked_mul(u(4))?
        .checked_div(p.price_out)?
        .checked_mul(p.price_in)?
        .checked_mul(fee_mul)?
        .checked_add(
            p.fictive_in
                .checked_mul(p.fictive_in)?
                .checked_mul(p.fees_lp.checked_mul(p.fees_lp)?)?,
        )?;
    let cmp = to_sub.checked_add(amount.checked_mul(to_div)?)?;
    if in_sqrt < cmp.checked_mul(cmp)? {
        return in_sqrt.root(2).checked_sub(to_sub).map(|x| x / to_div);
    }
    Some(amount)
}

/// computeFictiveReserves (L187-L209). Принимает: резервы и фиктивные резервы (вход, выход). Возвращает: новые фиктивные
/// резервы (вход, выход), делённые на 4. None — переполнение или деление на 0.
fn fictive(r_in: U256, r_out: U256, f_in: U256, f_out: U256) -> Option<(U256, U256)> {
    let (a, b) = if r_out.checked_mul(f_in)? < r_in.checked_mul(f_out)? {
        let temp = r_out
            .checked_mul(r_out)?
            .checked_div(f_out)?
            .checked_mul(f_in)?
            .checked_div(r_in)?;
        let a = temp
            .checked_mul(f_in)?
            .checked_div(f_out)?
            .checked_add(r_out.checked_mul(f_in)?.checked_div(f_out)?)?;
        (a, r_out.checked_add(temp)?)
    } else {
        let a = f_in
            .checked_mul(r_out)?
            .checked_div(f_out)?
            .checked_add(r_in)?;
        let b = r_in
            .checked_mul(f_out)?
            .checked_div(f_in)?
            .checked_add(r_out)?;
        (a, b)
    };
    Some((a / u(4), b / u(4)))
}

/// applyKConstRuleOut (L220-L245). Принимает: состояние стороны, вход (без вычета комиссий). Возвращает: (выход, новые
/// резервы вход/выход, новые фиктивные вход/выход). None — переполнение, выход больше резерва или фиктивного резерва.
fn k_out(p: &Params, amount: U256) -> Option<(U256, U256, U256, U256, U256)> {
    let base = u(FEES_BASE);
    let with_fee = amount.checked_mul(base.checked_sub(p.fees_lp)?.checked_sub(p.fees_pool)?)?;
    let num = with_fee.checked_mul(p.fictive_out)?;
    let den = p.fictive_in.checked_mul(base)?.checked_add(with_fee)?;
    let out = num.checked_div(den)?;
    let with_lp = with_fee.checked_add(amount.checked_mul(p.fees_lp)?)? / base;
    Some((
        out,
        p.reserve_in.checked_add(with_lp)?,
        p.reserve_out.checked_sub(out)?,
        p.fictive_in.checked_add(with_lp)?,
        p.fictive_out.checked_sub(out)?,
    ))
}

/// getAmountOut (L292-L406) со средней ценой, уже приведённой к времени блока (smardex_swap::price_average). Принимает: состояние
/// стороны, вход. Возвращает: (выход, новые фиктивные резервы вход/выход). None — как откат: нулевой вход / резерв,
/// переполнение, выход больше резерва.
pub fn amount_out(p: &Params, amount: U256) -> Option<(U256, U256, U256)> {
    amount_out_version(p, amount, false)
}

/// На Ethereum SmardexLibrary.approxEq явно принимает равные операнды даже при нулевом допуске.
/// Принимает параметры пула, входной объём и правило сравнения. Возвращает выход и фиктивные резервы
/// по правилам выбранной версии. При недопустимых входных данных возвращает None.
pub(super) fn amount_out_version(
    p: &Params,
    amount: U256,
    exact_match: bool,
) -> Option<(U256, U256, U256)> {
    let base = u(FEES_BASE);
    if amount.is_zero()
        || p.reserve_in.is_zero()
        || p.reserve_out.is_zero()
        || p.fictive_in.is_zero()
        || p.fictive_out.is_zero()
    {
        return None;
    }
    let net = base.checked_sub(p.fees_pool)?.checked_sub(p.fees_lp)?;
    let with_fees = amount.checked_mul(net)? / base;
    let first = first_in(p, with_fees)?;
    let mut s = *p;
    if first == with_fees
        && approx_eq(
            s.fictive_in.checked_mul(s.price_out)?,
            s.fictive_out.checked_mul(s.price_in)?,
            exact_match,
        )?
    {
        (s.fictive_in, s.fictive_out) =
            fictive(s.reserve_in, s.reserve_out, s.fictive_in, s.fictive_out)?;
    }
    let first_no_fees = first.checked_mul(base)?.checked_div(net)?;
    let (mut out, r_in, r_out, f_in, f_out) = k_out(&s, first_no_fees)?;
    let rest = amount.checked_sub(first_no_fees)?;
    if first >= with_fees {
        return Some((out, f_in, f_out));
    }
    let (f_in, f_out) = fictive(r_in, r_out, f_in, f_out)?;
    let second = Params {
        reserve_in: r_in,
        reserve_out: r_out,
        fictive_in: f_in,
        fictive_out: f_out,
        ..s
    };
    let (o2, _, _, f_in, f_out) = k_out(&second, rest)?;
    out = out.checked_add(o2)?;
    Some((out, f_in, f_out))
}
