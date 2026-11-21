//! Файл: venues/bsc_dex/math/fluid_swap.rs
//! Что делает: решение 440 — обмен с точным входом в пуле Fluid DEX T1 по прочитанному состоянию (PoolState::SmartColDebt)
//! — построчный порт `_swapIn` пула до операций со слоем ликвидности: приведение входа, проверка пределов суммы, комиссия
//! и доля протокола, доля пути через «смарт-залог» и «смарт-долг» (_swapRoutingIn), выход каждой части (_getAmountOut по
//! мнимым резервам), проверка остатка реальных резервов (_verifyToken0Reserves / _verifyToken1Reserves), новая цена пула
//! и обратное приведение выхода. Те же округления и те же места отката, что в Solidity 0.8 (checked — None; unchecked —
//! с переполнением по модулю, как EVM). Пределы слоя ликвидности и проверка скачка цены — math/fluid_check.rs.
//! Кто вызывает: quote/pool_quote.rs (ветка SmartColDebt), math/fluid_check.rs, тесты.
//! От чего зависит: core/types (SmartColDebtData), alloy-primitives (U256, I256).
//! Настройки: нет.
//!
//! Источник (решение 23): github.com/Instadapp/fluid-contracts-public@404d73f91e6806f881f8dabeb30494f93f9e99b7
//! contracts/protocols/dex/poolT1/coreModule/core/main.sol L51-L338 (_swapIn; откат-оценка FluidDexSwapResult — L338),
//! coreModule/helpers/coreHelpers.sol L76-L89 (_getAmountOut), L117-L130 (_swapRoutingIn), L172-L178 (_check),
//! L180-L204 (_verifyToken0Reserves / _verifyToken1Reserves), L206-L211 (_verifySwapAndNonPerfectActions);
//! common/constantVariables.sol L50-L58, L65, L72 (X96, X128, TWO/SIX/EIGHT_DECIMALS, 1e27, MINIMUM_LIQUIDITY_SWAP);
//! sqrt — solady FixedPointMathLib.sqrt (целая часть корня, как U256::root(2)).

use alloy_primitives::{I256, U256};

use crate::core::types::SmartColDebtData;

/// SIX_DECIMALS = 1e6 (constantVariables.sol L57): основание комиссии.
const SIX: u128 = 1_000_000;
/// EIGHT_DECIMALS = 1e8 (L58): основание доли протокола.
const EIGHT: u128 = 100_000_000;
/// TWO_DECIMALS = 1e2 (L53): наименьший вход в единицах токена.
const TWO: u128 = 100;
/// MINIMUM_LIQUIDITY_SWAP = 1e4 (L72).
const MIN_LIQ: u128 = 10_000;
/// Точность цены 1e27 (PRICE_PRECISION, L65; в _swapIn — литерал 1e27).
const E27: u128 = 1_000_000_000_000_000_000_000_000_000;
/// 1e18 — точность корня в _swapRoutingIn (coreHelpers.sol L126-L127).
const E18: u128 = 1_000_000_000_000_000_000;

/// Резервы одной части по направлению обмена (вход / выход), приведённые единицы.
#[derive(Debug, Clone, Copy, Default)]
pub struct Part {
    pub in_real: U256,
    pub out_real: U256,
    pub in_imag: U256,
    pub out_imag: U256,
}

/// Итог обмена: выход в единицах токена и то, что нужно проверкам fluid_check.rs.
#[derive(Debug, Clone, Copy)]
pub struct Out {
    /// Выход (единицы токена выхода) — то, что вернула бы оценка пула FluidDexSwapResult.
    pub amount_out: U256,
    /// Вход в части (приведённые единицы, ДО доли протокола) — как в резолвере (estimateSwapIn).
    pub in_col: U256,
    pub in_debt: U256,
    /// Выход частей (приведённые единицы).
    pub out_col: U256,
    pub out_debt: U256,
    /// Новая цена пула (1e27), которую пул передаёт в _updateOracle.
    pub price: U256,
    /// Резервы частей по направлению (для проверки скачка цены старых пулов).
    pub col: Part,
    pub debt: Part,
}

/// Константа протокола как U256. Ошибок нет.
fn u(n: u128) -> U256 {
    U256::from(n)
}

/// Часть по направлению. Принимает: включена ли, направление, резервы (real0, real1, imag0, imag1). Ошибок нет.
fn part(on: bool, swap0to1: bool, r: [U256; 4]) -> Part {
    if !on {
        return Part::default();
    }
    let (i, o) = if swap0to1 { (0, 1) } else { (1, 0) };
    Part {
        in_real: r[i],
        out_real: r[o],
        in_imag: r[2 + i],
        out_imag: r[2 + o],
    }
}

/// Резервы частей по направлению. Принимает: состояние, направление token0 → token1. Ошибок нет.
pub fn parts(d: &SmartColDebtData, swap0to1: bool) -> (Part, Part) {
    let c = &d.col;
    let b = &d.debt;
    let col = part(d.col_on, swap0to1, [c.real0, c.real1, c.imag0, c.imag1]);
    let debt = part(d.debt_on, swap0to1, [b.real0, b.real1, b.imag0, b.imag1]);
    (col, debt)
}

/// _getAmountOut (L76-L89, unchecked): вход·резерв_выхода / (резерв_входа + вход). None — деление на ноль (откат).
fn amount_out(a: U256, r_in: U256, r_out: U256) -> Option<U256> {
    a.wrapping_mul(r_out).checked_div(r_in.wrapping_add(a))
}

/// Целая часть корня из x·y·1e18 (L126-L127). None — переполнение (откат). Ошибок иных нет.
fn root(x: U256, y: U256) -> Option<U256> {
    let v = x.checked_mul(y)?.checked_mul(U256::from(E18))?;
    Some(v.root(2))
}

/// _swapRoutingIn (L117-L130): сколько входа идёт через залог (может быть < 0 или > t). None — откат (переполнение,
/// деление на ноль).
fn routing(t: U256, x: U256, y: U256, x2: U256, y2: U256) -> Option<I256> {
    let xy = root(x, y)?;
    let x2y2 = root(x2, y2)?;
    let a = y2.checked_mul(xy)?.checked_add(t.checked_mul(xy)?)?;
    let b = y.checked_mul(x2y2)?;
    let num = I256::from_raw(a).checked_sub(I256::from_raw(b))?;
    let den = I256::from_raw(xy.checked_add(x2y2)?);
    num.checked_div(den)
}

/// _verifyToken0Reserves (L180-L190): token0 после обмена не меньше token1·1e27/(центр·1e4). true — проходит.
fn ok0(t0: U256, t1: U256, cp: U256) -> Option<bool> {
    let den = cp.checked_mul(u(MIN_LIQ))?;
    Some(t0 >= t1.checked_mul(u(E27))?.checked_div(den)?)
}

/// _verifyToken1Reserves (L192-L204): token1 после обмена не меньше token0·центр/(1e27·1e4). true — проходит.
fn ok1(t0: U256, t1: U256, cp: U256) -> Option<bool> {
    let den = u(E27).wrapping_mul(u(MIN_LIQ));
    Some(t1 >= t0.checked_mul(cp)?.checked_div(den)?)
}

/// Выход одной части и проверка её реальных резервов (L249-L291). Принимает: вход части, комиссию (1e6 − fee), часть,
/// направление, центральную цену. Возвращает: выход части (приведённые) или None — откат пула.
fn leg(amt: U256, fee: U256, p: &Part, swap0to1: bool, cp: U256) -> Option<U256> {
    if amt.is_zero() {
        return Some(U256::ZERO);
    }
    let with_fee = amt.checked_mul(fee)? / u(SIX);
    let out = amount_out(with_fee, p.in_imag, p.out_imag)?;
    let in_after = p.in_real.checked_add(amt)?;
    let out_after = p.out_real.checked_sub(out)?;
    let pass = if swap0to1 {
        ok1(in_after, out_after, cp)?
    } else {
        ok0(out_after, in_after, cp)?
    };
    pass.then_some(out)
}

/// Новая цена части после обмена (L302-L312). None — откат (вычитание ниже нуля, деление на ноль).
fn price_of(p: &Part, inp: U256, out: U256, swap0to1: bool) -> Option<U256> {
    let rest_out = p.out_imag.checked_sub(out)?;
    let new_in = p.in_imag.checked_add(inp)?;
    if swap0to1 {
        rest_out.checked_mul(u(E27))?.checked_div(new_in)
    } else {
        new_in.checked_mul(u(E27))?.checked_div(rest_out)
    }
}

/// Делёж входа между залогом и долгом (L194-L247). Принимает: вход (приведённый), состояние, части. Возвращает:
/// (в залог, в долг) или None — откат (больше половины мнимых резервов входа, нет маршрута).
fn split(adj: U256, d: &SmartColDebtData, col: &Part, debt: &Part) -> Option<(U256, U256)> {
    if adj > col.in_imag.wrapping_add(debt.in_imag) / U256::from(2u8) {
        return None;
    }
    let r = if d.col_on && d.debt_on {
        routing(adj, col.out_imag, col.in_imag, debt.out_imag, debt.in_imag)?
    } else {
        I256::ZERO
    };
    let adj_i = I256::from_raw(adj);
    if adj_i > r && r > I256::ZERO {
        let c = r.into_raw();
        return Some((c, adj - c));
    }
    if (d.col_on && !d.debt_on) || r >= adj_i {
        return Some((adj, U256::ZERO));
    }
    if (!d.col_on && d.debt_on) || r <= I256::ZERO {
        return Some((U256::ZERO, adj));
    }
    None
}

/// Множители точности по направлению: (num входа, den входа, num выхода, den выхода). Ошибок нет.
fn scales(d: &SmartColDebtData, swap0to1: bool) -> [U256; 4] {
    if swap0to1 {
        [d.num0, d.den0, d.num1, d.den1]
    } else {
        [d.num1, d.den1, d.num0, d.den0]
    }
}

/// Обмен с точным входом (порт _swapIn, L51-L338). Принимает: состояние, направление (token0 пула → token1), вход в
/// единицах токена. Возвращает: Out или None — пул откатил бы обмен (пауза, не включён, пределы суммы, больше половины
/// мнимых резервов входа, нет маршрута, остаток резервов ниже предела, переполнение).
pub fn swap_in(d: &SmartColDebtData, swap0to1: bool, amount_in: U256) -> Option<Out> {
    if d.paused || (!d.col_on && !d.debt_on) {
        return None;
    }
    let [num_in, den_in, num_out, den_out] = scales(d, swap0to1);
    let one = U256::from(1u8);
    let x96 = (one << 96) - one;
    let x128 = (one << 128) - one;
    if amount_in > x128 {
        return None;
    }
    let adj = amount_in.wrapping_mul(num_in).checked_div(den_in)?;
    if !(u(SIX)..=x96).contains(&adj) || amount_in < u(TWO) {
        return None;
    }
    let fee_raw = U256::from(d.fee);
    // L95-L105 (unchecked): доля протокола 1e8 − cut·fee, комиссия 1e6 − fee.
    let fee = u(SIX).wrapping_sub(fee_raw);
    let cut_part = U256::from(d.revenue_cut).wrapping_mul(fee_raw);
    let cut = u(EIGHT).wrapping_sub(cut_part);
    let (col, debt) = parts(d, swap0to1);
    let (in_col, in_debt) = split(adj, d, &col, &debt)?;
    let cp = d.center_price;
    let out_col = leg(in_col, fee, &col, swap0to1, cp)?;
    let out_debt = leg(in_debt, fee, &debt, swap0to1, cp)?;
    let cut_col = in_col.checked_mul(cut)? / u(EIGHT);
    let cut_debt = in_debt.checked_mul(cut)? / u(EIGHT);
    let price = if cut_col > cut_debt {
        price_of(&col, cut_col, out_col, swap0to1)?
    } else {
        price_of(&debt, cut_debt, out_debt, swap0to1)?
    };
    // Обратное приведение (L314-L335): вход — checked (откат при делении на ноль), выход — unchecked.
    let back_in = |x: U256| x.checked_mul(den_in).and_then(|v| v.checked_div(num_in));
    if back_in(cut_col).is_none() || back_in(cut_debt).is_none() {
        return None;
    }
    let back = |x: U256| x.wrapping_mul(den_out).checked_div(num_out);
    let amount_out = back(out_col)?.wrapping_add(back(out_debt)?);
    Some(Out {
        amount_out,
        in_col,
        in_debt,
        out_col,
        out_debt,
        price,
        col,
        debt,
    })
}
