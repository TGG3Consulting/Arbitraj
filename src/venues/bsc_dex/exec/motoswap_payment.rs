//! Проверка суммы списания и выплаты владельцу по событиям пар, FeeRouter и модуля Motoswap.
//! Симулятор и обработчик квитанций сопоставляют переводы с плательщиком и путём из подписанного вызова.
use super::{Proof, Route, abi};
use crate::venues::bsc_dex::{check::simulate::SimLog, exec::{fill, signed_call}};
use alloy::{rpc::types::Log, sol_types::{SolEvent, SolValue}};
use alloy_primitives::{Address, U256};

fn unique<E: SolEvent>(logs: &[Log], addr: Address) -> Option<E> {
    let mut candidates = logs
        .iter()
        .filter(|l| l.address() == addr && l.topic0() == Some(&E::SIGNATURE_HASH));
    let event = E::decode_log_data(&candidates.next()?.inner.data).ok()?;
    candidates.next().is_none().then_some(event)
}

impl Route {
    pub(in crate::venues::bsc_dex) fn flows(
        &self,
        p: &Proof,
        logs: &[Log],
    ) -> Option<Vec<(U256, U256)>> {
        if self.pools.len() < 2
            || self.path.len() != self.pools.len() + 1
            || self.path.first() != Some(&p.input)
            || self.path.last() != Some(&p.output)
            || self.pools.first() != Some(&p.terms.pool)
        {
            return None;
        }
        let mut flows = Vec::with_capacity(self.pools.len());
        for (i, pool) in self.pools.iter().enumerate() {
            let e = unique::<abi::IPair::Swap>(logs, *pool)?;
            let forward = self.path[i] < self.path[i + 1];
            let (input, output) = if forward {
                (e.amount0In, e.amount1Out)
            } else {
                (e.amount1In, e.amount0Out)
            };
            let next = self.pools.get(i + 1).copied().unwrap_or(p.payer);
            if e.sender != p.terms.core
                || (e.to != next && e.to != p.terms.fee)
                || input.is_zero()
                || output.is_zero()
            {
                return None;
            }
            flows.push((input, output));
        }
        Some(flows)
    }
}

impl Proof {
    pub fn measured(&self, rows: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(rows)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub fn amounts(
        &self,
        logs: &[Log],
        payer: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = self.terms;
        if (payer, input, output) != (self.payer, self.input, self.output)
            || input == output
            || self.minimum.is_zero()
        {
            return None;
        }
        let e = unique::<abi::IFee::Swap>(logs, t.fee)?;

        let debit = fill::moved_logs(logs, input, Some(payer), None)
            .checked_sub(fill::moved_logs(logs, input, None, Some(payer)))?;
        let credit = fill::moved_logs(logs, output, None, Some(payer))
            .checked_sub(fill::moved_logs(logs, output, Some(payer), None))?;
        if debit != self.maximum
            || credit < self.minimum
            || (e.tokenIn, e.tokenOut) != (input, output)
            || e.amountIn.is_zero()
            || e.amountIn > debit
            || e.amountOut != credit
        {
            return None;
        }
        if let Some(route) = &self.route {
            if !route.path.contains(&e.feeToken) || e.user != t.target && t.module_hash.is_some() {
                return None;
            }
            route.flows(self, logs)?;
            if t.module_hash.is_some() {
                let paid = unique::<abi::IModule::PathPaid>(logs, t.target)?;
                let route_hash = alloy_primitives::keccak256(
                    (route.pools.clone(), route.path.clone()).abi_encode_params(),
                );
                if (
                    paid.route,
                    paid.payer,
                    paid.recipient,
                    paid.input,
                    paid.output,
                    paid.spent,
                    paid.received,
                ) != (route_hash, payer, payer, input, output, debit, credit)
                {
                    return None;
                }
            } else if e.user != payer || t.target != t.fee {
                return None;
            }
            return Some((debit, credit));
        }
        let swap = unique::<abi::IPair::Swap>(logs, t.pool)?;
        if ![input, output].contains(&e.feeToken)
            || swap.sender != t.core
            || ![payer, t.fee].contains(&swap.to)
        {
            return None;
        }
        let (pool_in, pool_out) = if input < output {
            (swap.amount0In, swap.amount1Out)
        } else {
            (swap.amount1In, swap.amount0Out)
        };
        if pool_in.is_zero() || pool_out.is_zero() {
            return None;
        }
        if t.module_hash.is_some() {
            let paid = unique::<abi::IModule::Paid>(logs, t.target)?;
            if (
                paid.pool,
                paid.payer,
                paid.recipient,
                paid.input,
                paid.output,
                paid.spent,
                paid.received,
            ) != (t.pool, payer, payer, input, output, debit, credit)
                || e.user != t.target
            {
                return None;
            }
        } else if t.target != t.fee || e.user != payer {
            return None;
        }
        Some((debit, credit))
    }
}
