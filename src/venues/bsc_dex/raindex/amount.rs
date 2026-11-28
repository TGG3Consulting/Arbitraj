//! Положительный Rain Float: знаковый 224-битный коэффициент и знаковый десятичный порядок.
//! RaindexV6 LibDecimalFloat.unpack. V4 quote использует 18 десятичных знаков для цены и ёмкости.
use alloy_primitives::{B256, U256, U512};
#[derive(Clone, Copy, Debug)]
pub(super) struct Quantity {
    coefficient: U256,
    exponent: i32,
}
impl Quantity {
    pub(super) fn v4(value: U256) -> Option<Self> {
        (!value.is_zero()).then_some(Self {
            coefficient: value,
            exponent: -18,
        })
    }
    pub(super) fn float(word: B256) -> Option<Self> {
        let b = word.as_slice();
        let c = U256::from_be_slice(&b[4..]);
        (b[4] & 128 == 0 && !c.is_zero()).then_some(Self {
            coefficient: c,
            exponent: i32::from_be_bytes(b[..4].try_into().ok()?),
        })
    }
    pub(super) fn units(amount: U256, dec: u8) -> Option<B256> {
        if amount.bit_len() > 223 {
            return None;
        }
        let mut word = amount.to_be_bytes::<32>();
        word[..4].copy_from_slice(&(-i32::from(dec)).to_be_bytes());
        Some(word.into())
    }
    fn scaled(self, amount: U256, delta: i32, invert: bool, up: bool) -> Option<U256> {
        let mut exp = i64::from(self.exponent);
        let (mut n, mut d) = if invert {
            exp = -exp;
            (U512::from(amount), U512::from(self.coefficient))
        } else {
            (U512::from(amount) * U512::from(self.coefficient), U512::ONE)
        };
        exp += i64::from(delta);
        if exp.unsigned_abs() > 154 {
            return None;
        }
        let power = U512::from(10).checked_pow(U512::from(exp.unsigned_abs()))?;
        if exp >= 0 {
            n = n.checked_mul(power)?;
        } else {
            d = d.checked_mul(power)?;
        }
        let (q, r) = n.div_rem(d);
        let raw = q
            .checked_add(if up && !r.is_zero() {
                U512::ONE
            } else {
                U512::ZERO
            })?
            .to_be_bytes::<64>();
        raw[..32]
            .iter()
            .all(|v| *v == 0)
            .then(|| U256::from_be_slice(&raw[32..]))
    }
    pub(super) fn cap(self, dec: u8) -> Option<U256> {
        self.scaled(U256::ONE, i32::from(dec), false, false)
    }
    pub(super) fn paid(self, received: U256, di: u8, do_: u8) -> Option<U256> {
        self.scaled(received, i32::from(di) - i32::from(do_), false, true)
    }
    pub(super) fn received(self, paid: U256, di: u8, do_: u8) -> Option<U256> {
        self.scaled(paid, i32::from(do_) - i32::from(di), true, false)
    }
}
/// Размер одиночного take. На пределе книги V6 целый дебет может включать округление
/// вверх при переводе ERC20. В этом случае фиксируем выход, а модуль проверяет точный дебет.
pub(super) fn fill_size(
    ratio: Quantity,
    capacity: Option<U256>,
    wanted: U256,
    decimals: (u8, u8),
    buy: bool,
    version: u8,
) -> Option<(U256, U256, bool)> {
    let (di, dout) = decimals;
    let (mut output, debit) = if buy {
        (wanted, ratio.paid(wanted, di, dout)?)
    } else {
        (ratio.received(wanted, di, dout)?, wanted)
    };
    let mut fixed_receive = buy || version <= 5;
    if let Some(cap) = capacity && output > cap {
        if buy || version != 6 || ratio.paid(cap, di, dout)? != debit {
            return None;
        }
        output = cap;
        fixed_receive = true;
    }
    (!output.is_zero() && !debit.is_zero()).then_some((output, debit, fixed_receive))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equivalent_vault_values_and_directional_rounding() {
        let a = Quantity::float(Quantity::units(U256::from(5), 1).unwrap()).unwrap();
        let b = Quantity::float(Quantity::units(U256::from(50), 2).unwrap()).unwrap();
        assert_eq!(a.cap(6), b.cap(6));
        assert_eq!(a.cap(6), Some(U256::from(500000)));
        assert_eq!(a.paid(U256::ONE, 6, 6), Some(U256::ONE));
        assert_eq!(a.received(U256::ONE, 6, 6), Some(U256::from(2)));
        assert!(Quantity::float(B256::repeat_byte(255)).is_none());
    }
}
