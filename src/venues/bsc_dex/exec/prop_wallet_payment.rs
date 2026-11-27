//! По квитанции или результату симуляции вычисляет чистые списание и выплату PropAMM.
//! Исполнитель и восстановление сверяют их с подписанным плательщиком, активами и источником выплаты.
use super::Proof;
use crate::venues::bsc_dex::{
    check::simulate::SimLog, exec::{fill, prop_abi::{IProp, IStrict, IWrappedProp}, signed_call},
    node::abi::IERC20,
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, U256};

impl Proof {
    pub(in crate::venues::bsc_dex) fn measured(&self, logs: &[SimLog]) -> Option<(U256, U256)> {
        let (_, paid, received) = self.outcome(&signed_call::logs(logs)?)?;
        Some((paid, received))
    }
    pub(in crate::venues::bsc_dex::exec) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (self.payer, self.input, self.output) != (wallet, input, output) {
            return None;
        }
        let (_, paid, received) = self.outcome(logs)?;
        Some((paid, received))
    }
    pub(in crate::venues::bsc_dex) fn outcome(
        &self,
        logs: &[Log],
    ) -> Option<(Address, U256, U256)> {
        let t = self.terms;
        let protected = t.target != t.router;
        let owner = self.payer;
        if self.input == self.output
            || owner == t.venue
            || owner == t.vault
            || owner == t.router
            || owner == t.fallback
            || owner == t.target
        {
            return None;
        }
        let sender = if protected { t.target } else { owner };
        let mut swaps = logs.iter().filter(|l| {
            l.address() == t.router && l.topic0() == Some(&IProp::Swapped::SIGNATURE_HASH)
        });
        let s = IProp::Swapped::decode_log_data(&swaps.next()?.inner.data).ok()?;
        if swaps.next().is_some()
            || (s.sender, s.recipient, s.tokenIn, s.tokenOut)
                != (sender, owner, self.input, self.output)
            || s.amountOut.is_zero()
            || s.amountOut < self.minimum
            || s.amountIn.is_zero()
        {
            return None;
        }
        let moved = |coin, from, to| fill::moved_logs(logs, coin, from, to);
        let debit = moved(self.input, Some(owner), None).checked_sub(moved(
            self.input,
            None,
            Some(owner),
        ))?;
        let credit = moved(self.output, None, Some(owner)).checked_sub(moved(
            self.output,
            Some(owner),
            None,
        ))?;
        if debit != self.amount || credit != s.amountOut || debit < s.amountIn {
            return None;
        }
        if protected {
            let mut events = logs.iter().filter(|l| {
                l.address() == t.target && l.topic0() == Some(&IStrict::Filled::SIGNATURE_HASH)
            });
            let f = IStrict::Filled::decode_log_data(&events.next()?.inner.data).ok()?;
            if events.next().is_some()
                || (
                    f.payer,
                    f.recipient,
                    f.venue,
                    f.input,
                    f.output,
                    f.spent,
                    f.received,
                ) != (
                    owner,
                    owner,
                    s.marketMaker,
                    self.input,
                    self.output,
                    debit,
                    credit,
                )
                || moved(self.input, Some(owner), Some(t.target)) != s.amountIn
            {
                return None;
            }
        } else if debit != s.amountIn {
            return None;
        }
        if s.marketMaker == t.venue {
            let forwarded = |a, b| moved(self.input, Some(a), Some(b)) == s.amountIn;
            let venue_swap = || -> Option<bool> {
                let mut emitted = logs.iter().filter(|l| {
                    l.address() == t.venue
                        && l.topic0() == Some(&IWrappedProp::Swapped::SIGNATURE_HASH)
                });
                let fill =
                    IWrappedProp::Swapped::decode_log_data(&emitted.next()?.inner.data).ok()?;
                Some(
                    emitted.next().is_none()
                        && (
                            fill.sender,
                            fill.tokenIn,
                            fill.tokenOut,
                            fill.amountIn,
                            fill.amountOut,
                            fill.recipient,
                        ) == (t.router, self.input, self.output, s.amountIn, credit, owner),
                )
            };
            let path_matches = match t.via {
                Some([pool, logic]) => {
                    venue_swap() == Some(true)
                        && forwarded(t.venue, pool)
                        && forwarded(pool, logic)
                        && forwarded(logic, t.vault)
                }
                None => t.venue == t.vault || forwarded(t.venue, t.vault),
            };
            let direct = moved(self.output, Some(t.vault), Some(owner)) == credit;
            // Kaliber и Taurus могут платить через рынок: сверяем оба перевода, событие
            // расчёта рынка и отсутствие у него остатка от этой выплаты.
            let relayed = !direct
                && t.via.is_none()
                && t.venue != t.vault
                && t.venue != owner
                && venue_swap() == Some(true)
                && moved(self.output, Some(t.vault), Some(t.venue)) == credit
                && moved(self.output, Some(t.venue), Some(owner)) == credit
                && moved(self.output, None, Some(t.venue)) == credit
                && moved(self.output, Some(t.venue), None) == credit;
            if !forwarded(sender, t.venue) || !path_matches || !(direct || relayed) {
                return None;
            }
        } else if s.marketMaker == t.fallback && t.fallback_allowed {
            if moved(self.input, Some(sender), Some(t.router)) != s.amountIn {
                return None;
            }
            // Пара Uniswap получает вход от прокси и сама отправляет выход получателю.
            let mut pools = logs
                .iter()
                .filter(|l| l.address() == self.output)
                .filter_map(|l| IERC20::Transfer::decode_log_data(&l.inner.data).ok())
                .filter(|tr| tr.to == owner && !tr.value.is_zero());
            let pool = pools.next()?;
            if pools.next().is_some()
                || pool.value != credit
                || moved(self.input, Some(t.router), Some(pool.from)) != s.amountIn
            {
                return None;
            }
        } else {
            return None;
        }
        Some((s.marketMaker, debit, credit))
    }
}
