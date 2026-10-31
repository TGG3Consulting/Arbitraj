//! Файл: venues/bsc_dex/math/swap_solidly.rs
//! Что делает: уровень свопа СТАБИЛЬНОГО пула Solidly (решение 80 — экзотическая математика ve(3,3)-форков) — порт
//! `getAmountOut`/`_getAmountOut` официального контракта для стабильного пула (инвариант `x³y + xy³ = k`): вычесть
//! комиссию из входа, нормализовать резервы и вход к 1e18 по decimals каждого токена, решить новый баланс встречного
//! токена методом Ньютона (math/solidly_math.rs) и денормализовать выход. Сам инвариант (f/d/get_y) — в
//! math/solidly_math.rs. Эта математика закрывает стабильные пулы ve(3,3)-форков на BSC (Topaz v2-stable, THENA V1
//! stable). ТОЛЬКО стабильный инвариант: волатильные пулы тех же форков — обычный `x*y=k` (V2), уже покрыт math/v2.rs.
//! Это уровень цены ОДНОГО свопа в одном плоском стабильном пуле; граф/маршрут/сплит (решение 80), чтение состояния
//! пула, события, квотер и исполнение, а также подключение семейств (Topaz/THENA V1) — следующая фаза.
//! Кто вызывает: quote/pool_quote.rs (через семейство Solidly, фаза B); тесты (tests/bsc_dex_solidly_math.rs).
//! От чего зависит: math/solidly_math.rs. Настройки: нет (резервы, decimals, комиссия — с контракта пула в фазе B).
//!
//! Источник (официальный, решение 23): velodrome-finance/contracts (HEAD b3065d8b6702b14b094f9f6046b752cc9f78c43b),
//! contracts/Pool.sol (pragma >=0.8.19): `getAmountOut`, `_getAmountOut` (ветка stable), `_k` (ветка stable).
//! Solidity 0.8 — вся арифметика checked → порт на checked_*, где контракт откатился бы (переполнение, недобор,
//! деление на ноль) — `None`.

use alloy_primitives::U256;

// Инвариант и константы (math/solidly_math.rs) — для котировки (фаза B) и тестов — доступны через этот модуль.
pub use super::solidly_math::{d, e18, f, fee_denominator, get_y};

/// Pool._k (ветка stable): инвариант `x³y + xy³` по РАЗНЫМ decimals токенов. Нормализует x,y к 1e18 по своим decimals
/// и считает кривую (`_x·_y/1e18 · (_x²/1e18 + _y²/1e18) / 1e18`) — это тождественно `f` над нормализованными x,y.
/// Принимает: x, y — резервы в единицах токена; dec_x, dec_y — 10^decimals каждого токена. Возвращает: инвариант k.
/// None — dec_x/dec_y = 0 (деление) или переполнение (в контракте checked откатился бы).
pub fn k(x: U256, y: U256, dec_x: U256, dec_y: U256) -> Option<U256> {
    if dec_x.is_zero() || dec_y.is_zero() {
        return None;
    }
    let one = e18();
    let xn = x.checked_mul(one)? / dec_x;
    let yn = y.checked_mul(one)? / dec_y;
    f(xn, yn)
}

/// Pool.getAmountOut (ветка stable): сколько встречного токена получит трейдер за `amount_in`, с комиссией пула.
/// Контракт: сначала `amount_in -= amount_in·fee/10000`, затем `_getAmountOut`: инвариант `xy = _k(resIn, resOut)` по
/// исходным резервам, резервы и вход нормализуются к 1e18 по decimals, `y = resOut_norm − get_y(in_norm + resIn_norm,
/// xy, resOut_norm)`, выход денормализуется `y·dec_out/1e18`.
/// Принимает: amount_in — вход в единицах входного токена; reserve_in, reserve_out — резервы пула в единицах токенов;
/// decimals_in, decimals_out — 10^decimals входного и выходного токенов; fee — числитель комиссии над 10000
/// (как getFee контракта). Возвращает: выход в единицах выходного токена.
/// None — decimals = 0, переполнение/недобор любой операции, несходимость Ньютона (см. solidly_math::get_y).
pub fn get_amount_out(
    amount_in: U256,
    reserve_in: U256,
    reserve_out: U256,
    decimals_in: U256,
    decimals_out: U256,
    fee: U256,
) -> Option<U256> {
    if decimals_in.is_zero() || decimals_out.is_zero() {
        return None;
    }
    let one = e18();
    // amount_in -= amount_in·fee/10000 (контракт убирает комиссию из полученного до расчёта).
    let fee_part = amount_in.checked_mul(fee)? / fee_denominator();
    let amount_in = amount_in.checked_sub(fee_part)?;
    // Инвариант до свопа по исходным резервам (нормализация по decimals — внутри k).
    let xy = k(reserve_in, reserve_out, decimals_in, decimals_out)?;
    // Нормализация резервов и входа к 1e18 по своим decimals.
    let res_in_n = reserve_in.checked_mul(one)? / decimals_in;
    let res_out_n = reserve_out.checked_mul(one)? / decimals_out;
    let in_n = amount_in.checked_mul(one)? / decimals_in;
    // Новый нормализованный баланс встречного токена и выход как его убыль.
    let new_y = get_y(in_n.checked_add(res_in_n)?, xy, res_out_n)?;
    let y = res_out_n.checked_sub(new_y)?;
    // Денормализация выхода в единицы выходного токена.
    Some(y.checked_mul(decimals_out)? / one)
}
