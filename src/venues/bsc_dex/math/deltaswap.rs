//! Точная комиссия обычного плательщика DeltaSwapPair. Обе EMA округляются по отдельным слагаемым.
use super::{swap_cl::Swapped, v2};
use crate::core::types::{PoolState, pool::DeltaFee};
use alloy_primitives::U256;

fn ema(now: U256, last: U256, weight: u64) -> Option<U256> {
    if last.is_zero() {
        return Some(now);
    }
    let weight = weight.min(100);
    (now.checked_mul(U256::from(weight))? / U256::from(100))
        .checked_add(last.checked_mul(U256::from(100 - weight))? / U256::from(100))
}

/// DS math computes half the input and its reserve-equivalent before the integer square root.
fn transition(f: &DeltaFee, rin: U256, rout: U256, dx: U256) -> Option<(u8, U256, U256)> {
    let half = dx / U256::from(2);
    let equivalent = half.checked_mul(rout)?.checked_div(rin)?;
    let liquidity = half.checked_mul(equivalent)?.root(2);
    if liquidity.is_zero() {
        return None;
    }
    let diff = f.block.checked_sub(u64::from(f.trade_block))?;
    let sum = if diff == 0 {
        f.sum.checked_add(liquidity)?
    } else {
        liquidity
    };
    let last = if diff > 50 { U256::ZERO } else { f.traded };
    let traded = ema(sum, last, 20)?;
    let threshold = f.liquidity.checked_mul(U256::from(f.threshold))? / U256::from(1000);
    let fee = if liquidity.max(traded) >= threshold {
        f.rate
    } else {
        0
    };
    Some((fee, traded, sum))
}

pub(crate) fn rate(f: &DeltaFee, rin: U256, rout: U256, dx: U256) -> Option<u8> {
    transition(f, rin, rout, dx).map(|(rate, _, _)| rate)
}

fn fraction(rate: u8) -> (U256, U256) {
    (U256::from(1000u16 - u16::from(rate)), U256::from(1000))
}

fn out(f: &DeltaFee, rin: U256, rout: U256, dx: U256) -> Option<U256> {
    let max = (U256::ONE << 112usize) - U256::ONE;
    if rin.checked_add(dx)? > max {
        return None;
    }
    let (fee, _, _) = transition(f, rin, rout, dx)?;
    v2::amount_out(dx, rin, rout, fraction(fee)).filter(|out| !out.is_zero())
}

pub fn quote(
    r0: U256,
    r1: U256,
    f: &DeltaFee,
    zfo: bool,
    exact_in: bool,
    amount: U256,
) -> Option<Swapped> {
    let (rin, rout) = if zfo { (r0, r1) } else { (r1, r0) };
    let (input, output) = if exact_in {
        (amount, out(f, rin, rout, amount)?)
    } else {
        // В точке порога ставка может вырасти скачком. Проверяем обе ветви и берём
        // меньший полный вход, достаточный при ставке, реально действующей на нём.
        if amount.is_zero() || amount >= rout {
            return None;
        }
        let min_trade = rin
            .div_ceil(rout)
            .max(U256::ONE)
            .checked_mul(U256::from(2))?;
        let candidates = [0, f.rate].map(|fee| {
            let (num, den) = fraction(fee);
            let need = rin
                .checked_mul(amount)?
                .checked_mul(den)?
                .div_ceil((rout - amount).checked_mul(num)?);
            let dx = need.max(min_trade);
            (out(f, rin, rout, dx)? >= amount).then_some(dx)
        });
        (candidates.into_iter().flatten().min()?, amount)
    };
    Some(Swapped {
        amount_in: input,
        amount_out: output,
        complete: true,
    })
}

pub fn after(
    r0: U256,
    r1: U256,
    f: &DeltaFee,
    zfo: bool,
    dx: U256,
) -> Option<(Swapped, PoolState)> {
    let (rin, rout) = if zfo { (r0, r1) } else { (r1, r0) };
    let s = quote(r0, r1, f, zfo, true, dx)?;
    let (_, traded, sum) = transition(f, rin, rout, dx)?;
    let (input, output) = (rin.checked_add(dx)?, rout.checked_sub(s.amount_out)?);
    let (reserve0, reserve1) = if zfo {
        (input, output)
    } else {
        (output, input)
    };
    let block = u32::try_from(f.block).ok()?;
    let diff = block.checked_sub(f.liquidity_block)?;
    let liquidity = if diff == 0 {
        f.liquidity
    } else {
        ema(
            reserve0.checked_mul(reserve1)?.root(2),
            f.liquidity,
            u64::from(diff.max(10)),
        )?
    };
    let mask = (U256::ONE << 112usize) - U256::ONE;
    let fee = Box::new(DeltaFee {
        traded: traded & mask,
        sum: sum & mask,
        trade_block: block,
        liquidity: liquidity & mask,
        liquidity_block: block,
        ..f.clone()
    });
    Some((
        s,
        PoolState::DeltaSwap {
            reserve0,
            reserve1,
            fee,
        },
    ))
}
