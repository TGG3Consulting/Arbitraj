//! Сверяет протокольный LogSwap, выплату модуля и чистые движения средств кошелька в одной квитанции.
use super::{
    fill,
    fluid_lite_wallet::{Proof, abi, scale},
};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, U256};

impl Proof {
    pub(in crate::venues::bsc_dex) fn amounts(
        &self,
        logs: &[Log],
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (payer, input, output) != (self.payer, self.input, self.output)
            || self.minimum.is_zero()
            || input == output
        {
            return None;
        }
        if let Some(route) = &self.route {
            return route.amounts(self, logs);
        }
        let mut rows = logs.iter().filter(|v| {
            v.address() == self.terms.target
                && v.topic0() == Some(&abi::IWallet::Filled::SIGNATURE_HASH)
        });
        let ev = abi::IWallet::Filled::decode_log_data(&rows.next()?.inner.data).ok()?;
        if rows.next().is_some()
            || (ev.market, ev.payer, ev.recipient, ev.input, ev.output)
                != (self.terms.market.id()?, payer, payer, input, output)
            || ev.spent.is_zero()
            || ev.spent > self.maximum
            || ev.received < self.minimum
        {
            return None;
        }
        let mut trades = logs.iter().filter(|l| {
            l.address() == self.terms.dex
                && l.topic0() == Some(&abi::IWallet::LogSwap::SIGNATURE_HASH)
        });
        let event = abi::IWallet::LogSwap::decode_log_data(&trades.next()?.inner.data).ok()?;
        let market = self.terms.market;
        let pair = market.tokens(self.terms.wrapped)?;
        let forward = (input, output) == (pair[0], pair[1]);
        if trades.next().is_some()
            || (!forward && (input, output) != (pair[1], pair[0]))
            || event.swapData.bit(64) != forward
            || (event.swapData & U256::from(u64::MAX)) != U256::from_be_slice(&market.id()?[..8])
            || (self.terms.desired.is_none() && ev.spent != self.maximum)
        {
            return None;
        }
        // The core emits both sides at nine decimals. The fixed leg is truncated into that
        // precision first; the computed leg is truncated back into its underlying token.
        let mask = (U256::ONE << 60) - U256::ONE;
        let ix = if forward { 126 } else { 131 };
        let ox = if forward { 131 } else { 126 };
        let decimals = |shift: usize| ((event.dexVariables >> shift) & U256::from(31)).to::<u8>();
        let spent9 = (event.swapData >> 65) & mask;
        let received9 = (event.swapData >> 125) & mask;
        if self.terms.desired.is_some() {
            if scale(spent9, 9, decimals(ix))? != ev.spent
                || scale(self.minimum, decimals(ox), 9)? != received9
            {
                return None;
            }
        } else if scale(self.maximum, decimals(ix), 9)? != spent9
            || scale(received9, 9, decimals(ox))?.is_zero()
        {
            return None;
        }
        let moved = |t, f, to| fill::moved_logs(logs, t, f, to);
        let debit = moved(input, Some(payer), None).checked_sub(moved(input, None, Some(payer)))?;
        let net = moved(output, None, Some(payer)).checked_sub(moved(output, Some(payer), None))?;
        let native = Address::repeat_byte(0xee);
        let (pin, pout) = if forward {
            (market.token0, market.token1)
        } else {
            (market.token1, market.token0)
        };
        let origin = if pout == native {
            self.terms.target
        } else {
            self.terms.dex
        };
        let payee = if pin == native {
            self.terms.target
        } else {
            self.terms.dex
        };
        (moved(input, Some(payer), Some(payee)) > U256::ZERO
            && moved(output, Some(origin), Some(payer)) > U256::ZERO
            && (debit, net) == (ev.spent, ev.received))
            .then_some((debit, net))
    }
}
