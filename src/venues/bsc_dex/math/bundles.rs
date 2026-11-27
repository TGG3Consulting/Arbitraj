//! Чистая выплата Bundles Swap и фактические остатки пары после сжигания и начисления дивидендов.
pub use crate::core::types::bundles::{Book, Market};
use alloy_primitives::{Address, U256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub credit: U256,
    pub bun_spent: U256,
    pub market: Market,
    pub fee_pair: Market,
}

fn portion(n: U256, rate: u16, div: u32) -> Option<U256> {
    Some(n.checked_mul(U256::from(rate))? / U256::from(div))
}

impl Market {
    pub(super) fn ceiling_fee(
        &self,
        i: usize,
        amount: U256,
        output: U256,
        embedded: bool,
    ) -> Option<U256> {
        let tier = self.tier;
        let k = 1000 + u32::from(tier);
        if embedded {
            return if i == 0 {
                portion(amount, tier, k)
            } else {
                portion(output, tier, 1000)
            };
        }
        let (side, rate) = self.oracle?;
        let value = if i == side {
            portion(amount, tier, 1000)?
        } else {
            portion(output, tier, k)?
        };
        Some(rate.checked_mul(value)? >> 112)
    }

    pub(super) fn price(&self, i: usize, n: U256, embedded: bool) -> Option<U256> {
        let k = U256::from(1000);
        let t = U256::from(self.tier);
        let net = if embedded && i == 0 {
            n.checked_mul(k)? / (k + t)
        } else {
            n
        };
        let result = net
            .checked_mul(self.reserves[1 - i])?
            .checked_div(self.reserves[i].checked_add(net)?)?;
        let out = if embedded && i == 1 {
            result.checked_mul(k)? / (k + t)
        } else {
            result
        };
        (!out.is_zero() && out < self.reserves[1 - i]).then_some(out)
    }

    /// Проверяет обе физические ноги Pair.swap; сборы вычисляются из действительного прихода.
    fn settle(&self, i: usize, n: U256, out: U256, paid: U256, embedded: bool) -> Option<Self> {
        if out.is_zero() || out >= self.reserves[1 - i] || self.supply <= U256::from(1000) {
            return None;
        }
        let mut next = self.clone();
        next.balances[i] = next.balances[i].checked_add(n)?;
        next.balances[1 - i] = next.balances[1 - i].checked_sub(out)?;
        let mut net = [next.balances[0], next.balances[1]];
        if embedded {
            net[0] = net[0].checked_sub(self.fee_reserve)?
        }
        let arrived = std::array::from_fn::<_, 2, _>(|k| {
            net[k].saturating_sub(self.reserves[k] - if k == i { U256::ZERO } else { out })
        });
        if arrived.iter().all(|x| x.is_zero()) {
            return None;
        }
        let tier = self.tier;
        let fee = if embedded {
            portion(arrived[0], tier, 1000 + u32::from(tier))?.checked_add(portion(
                if i == 1 { out } else { U256::ZERO },
                tier,
                1000,
            )?)?
        } else {
            let (side, rate) = self.oracle?;
            let q = portion(arrived[side], tier, 1000)?.checked_add(portion(
                if side == i { U256::ZERO } else { out },
                tier,
                1000 + u32::from(tier),
            )?)?;
            rate.checked_mul(q)? >> 112
        };
        if fee < U256::from(1000) {
            return None;
        }
        if embedded {
            net[0] = net[0].checked_sub(fee)?
        } else {
            next.balances[2] = next.balances[2].checked_add(paid)?;
            if next.balances[2].checked_sub(self.fee_reserve)? < fee {
                return None;
            }
        }
        if net.iter().any(|x| x.bit_len() > 112)
            || net[0].checked_mul(net[1])? < self.reserves[0].checked_mul(self.reserves[1])?
        {
            return None;
        }
        let dividend = portion(fee, 8500, 10000)?;
        next.fee_reserve = next.fee_reserve.checked_add(dividend)?;
        let burn = fee - dividend;
        if embedded {
            next.balances[0] = next.balances[0].checked_sub(burn)?;
            next.balances[2] = next.balances[0];
        } else {
            next.balances[2] = next.balances[2].checked_sub(burn)?
        }
        next.reserves = net;
        Some(next)
    }
}

impl Book {
    /// source=tokenIn. При оплате комиссией кошелька bun_spent учитывается отдельным расходом.
    pub fn exact_in(&self, source: Address, n: U256, separate: bool) -> Option<Fill> {
        let m = &self.market;
        let i = m.tokens.iter().position(|&t| t == source)?;
        let embedded = m.tokens[0] == self.bun;
        let output = m.price(i, n, embedded)?;
        let fee = m.ceiling_fee(i, n, output, embedded)?;
        if embedded || separate {
            let fee_debit = if embedded { U256::ZERO } else { fee };
            let next = m.settle(i, n, output, fee_debit, embedded)?;
            return Some(Fill {
                credit: output,
                bun_spent: fee_debit,
                fee_pair: if m.pool == self.fee_pair.pool {
                    next.clone()
                } else {
                    self.fee_pair.clone()
                },
                market: next,
            });
        }
        let f = &self.fee_pair;
        if f.tokens != [self.bun, self.weth] || f.tier != 10 || !m.tokens.contains(&self.weth) {
            return None;
        }
        // Встроенный _getAmountsIn Router увеличивает gross на 10/1000, затем всегда прибавляет 1 raw WETH.
        let cost = self.refill_cost(fee)?;
        let (principal, credit, wrapped, fee) = if source == self.weth {
            let principal = n.checked_mul(n)?.checked_div(n.checked_add(cost)?)?;
            let credit = m.price(i, principal, false)?;
            (
                principal,
                credit,
                n - principal,
                m.ceiling_fee(i, principal, credit, false)?,
            )
        } else {
            if self.deposits < fee {
                return None;
            }
            (n, output.checked_sub(cost)?, cost, fee)
        };
        if self.router_balance < fee || credit.is_zero() {
            return None;
        }
        let nominal = if source == self.weth { credit } else { output };
        let next = m.settle(i, principal, nominal, fee, false)?;
        let refill = f.settle(1, wrapped, fee, U256::ZERO, true)?;
        Some(Fill {
            credit,
            bun_spent: U256::ZERO,
            market: next,
            fee_pair: refill,
        })
    }
}
