//! Файл: venues/bsc_dex/math/maverick_swap_math.rs
//! Что делает: обмен ВНУТРИ одного тика Maverick V2 (решение 122 F) — точный вход и точный выход, комиссия с входа и
//! доля протокола. Порт команды Maverick, Solidity-исходник закрыт (решение Тиграна 03.11): исходник пула v2-amm
//! (poollib/SwapMath.sol) — в приватном maverickprotocol/maverick-v2; портировано с TS-порта, написанного командой
//! Maverick, — VeloraDEX/paraswap-dex-lib 4f74b83bb14d9b2c951a7cda673f0cca29dcad81,
//! src/dex/maverick-v2/maverick-math/maverick-swap-math.ts (первый коммит a1f86b88 — Trung Dinh, правка 2108e064 — Bob
//! Baxley, Maverick). Строки — у каждой функции. Базовые операции — официальный порт Math.sol (math/maverick_math.rs):
//! mulDivDown/Up одинарной ширины (переполнение x·y → None, как откат), divUp/Down, mulDown, invFloor/Ceil, clip.
//! Вычитания, которые в Solidity 0.8 откатились бы (checked), → checked_sub (None). Чистая математика без сети.
//! Кто вызывает: math/maverick_swap.rs.
//! От чего зависит: math/maverick_math.rs, alloy-primitives.
//! Настройки: нет.

use alloy_primitives::U256;

use super::maverick_math::{
    clip, div_up, inv_ceil, inv_floor, mul_div_down, mul_div_up, mul_down, one,
};

/// Резервы и ликвидность тика (TickData: currentReserveA/B, currentLiquidity) — D18 пула.
#[derive(Debug, Clone, Copy)]
pub struct TickData {
    pub reserve_a: U256,
    pub reserve_b: U256,
    pub liquidity: U256,
}

/// Итог обмена в тике (поля Delta, нужные суммам; fractionalPart — только для TWAP, не считается).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Step {
    /// Вход, оставшийся в тике (за вычетом доли протокола).
    pub in_bin_internal: U256,
    /// Вход с комиссией (то, что платит пользователь за этот тик).
    pub in_erc: U256,
    /// Выход из тика.
    pub out_erc: U256,
    /// Остаток заказа, не исполненный в этом тике (0 — тик последний).
    pub excess: U256,
}

/// amountToBinNetOfProtocolFee (maverick-swap-math.ts L8-L19): вход минус ceil(feeBasis·protocolFeeD3/1000).
/// None — переполнение.
pub fn net_of_protocol_fee(in_erc: U256, fee_basis: U256, protocol_d3: u8) -> Option<U256> {
    if protocol_d3 == 0 {
        return Some(in_erc);
    }
    let p = mul_div_up(fee_basis, U256::from(protocol_d3), U256::from(1000u32))?;
    Some(clip(in_erc, p))
}

/// remainingBinInputSpaceGivenOutput (maverick-swap-math.ts L21-L42): вход в тик, нужный на выход `output`.
/// None — переполнение или отрицательный знаменатель (откат в Solidity 0.8).
pub fn input_for_output(l: U256, output: U256, sqrt_price: U256, a_in: bool) -> Option<U256> {
    let out_over_l = if l.is_zero() {
        U256::ZERO
    } else {
        div_up(output, l)?
    };
    if a_in {
        let den = inv_floor(sqrt_price)?.checked_sub(out_over_l)?;
        mul_div_up(output, sqrt_price, den)
    } else {
        let den = mul_down(sqrt_price, sqrt_price.checked_sub(out_over_l)?)?;
        div_up(output, den)
    }
}

/// computeSwapExactIn (maverick-swap-math.ts L44-L122). Принимает: sqrt-цену в тике, тик, вход (D18), направление,
/// комиссию D18, долю протокола. None — переполнение / откат.
pub fn exact_in(
    sqrt_price: U256,
    t: TickData,
    amount_in: U256,
    a_in: bool,
    fee: U256,
    protocol_d3: u8,
) -> Option<Step> {
    let mut d = Step {
        out_erc: if a_in { t.reserve_b } else { t.reserve_a },
        ..Step::default()
    };
    let mut bin_in = input_for_output(t.liquidity, d.out_erc, sqrt_price, a_in)?;
    let keep = one().checked_sub(fee)?;
    let user_bin_in = mul_down(amount_in, keep)?;
    let fee_basis;
    if user_bin_in < bin_in {
        bin_in = user_bin_in;
        d.in_erc = amount_in;
        fee_basis = d.in_erc.checked_sub(user_bin_in)?;
    } else {
        fee_basis = mul_div_up(bin_in, fee, keep)?;
        d.in_erc = bin_in.checked_add(fee_basis)?;
        d.excess = clip(amount_in, d.in_erc);
    }
    d.in_bin_internal = net_of_protocol_fee(d.in_erc, fee_basis, protocol_d3)?;
    if !d.excess.is_zero() {
        return Some(d);
    }
    // L107-L119: выход по входу, вниз, не больше резерва тика.
    let in_over_l = div_up(bin_in, t.liquidity.checked_add(U256::from(1u8))?)?;
    let (num, add) = if a_in {
        (inv_floor(sqrt_price)?, sqrt_price)
    } else {
        (sqrt_price, inv_ceil(sqrt_price)?)
    };
    let by_in = mul_div_down(bin_in, num, in_over_l.checked_add(add)?)?;
    d.out_erc = d.out_erc.min(by_in);
    Some(d)
}

/// computeSwapExactOut (maverick-swap-math.ts L124-L176). Принимает: как exact_in, но выход (D18). None — переполнение
/// / откат.
pub fn exact_out(
    sqrt_price: U256,
    t: TickData,
    amount_out: U256,
    a_in: bool,
    fee: U256,
    protocol_d3: u8,
) -> Option<Step> {
    let available = if a_in { t.reserve_b } else { t.reserve_a };
    let swapped = available <= amount_out;
    let out_erc = if swapped { available } else { amount_out };
    let bin_in = input_for_output(t.liquidity, out_erc, sqrt_price, a_in)?;
    let fee_basis = mul_div_up(bin_in, fee, one().checked_sub(fee)?)?;
    let in_erc = bin_in.checked_add(fee_basis)?;
    Some(Step {
        in_bin_internal: net_of_protocol_fee(in_erc, fee_basis, protocol_d3)?,
        in_erc,
        out_erc,
        excess: if swapped {
            clip(amount_out, out_erc)
        } else {
            U256::ZERO
        },
    })
}
