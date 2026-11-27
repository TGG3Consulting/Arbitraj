//! Сверка включённого поручения Bond и обеих сторон расчёта при штатном подтверждении и восстановлении.
use super::super::{ctx::Ctx, node::rpc, pools::mint_club::IBond, send::track::Receipt};
use super::{fill, mint_club_wire::Call};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub call: Call,
    pub bond: Address,
    pub payer: Address,
}

impl Proof {
    pub(crate) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tx {
            hash: B256,
            block_hash: Option<B256>,
            from: Address,
            to: Option<Address>,
            value: U256,
            input: Bytes,
        }
        let fail = || {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Mint Club: не подтверждены исходный хеш, блок или подписанное поручение Bond",
            )
        };
        if r.transaction_hash != Some(hash) || r.block_hash.is_none() {
            return Err(fail());
        }
        let tx: Option<Tx> = rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
            .await
            .map_err(|_| fail())?;
        if tx.is_some_and(|t| {
            t.hash == hash
                && t.block_hash == r.block_hash
                && t.from == self.payer
                && t.to == Some(self.call.protection.map_or(self.bond, |p| p.module))
                && t.value.is_zero()
                && t.input == self.call.data(self.payer)
        }) {
            Ok(())
        } else {
            Err(fail())
        }
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        rows: &[super::super::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        let logs = rows
            .iter()
            .map(|r| {
                Some(Log {
                    inner: alloy_primitives::Log::new(r.address, r.topics.clone(), r.data.clone())?,
                    ..Default::default()
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let (input, output) = self.call.pair();
        self.amounts(&logs, self.payer, input, output)
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let c = self.call;
        if c.qty.is_zero()
            || c.limit.is_zero()
            || wallet.is_zero()
            || self.bond == wallet
            || wallet != self.payer
            || c.pair() != (input, output)
            || input == output
        {
            return None;
        }
        let mut rows=logs.iter().filter(|l|l.address()==self.bond && matches!(l.topic0(),Some(t) if *t==IBond::Mint::SIGNATURE_HASH||*t==IBond::Burn::SIGNATURE_HASH));
        let event = &rows.next()?.inner.data;
        if rows.next().is_some() {
            return None;
        }
        let (token, user, receiver, qty, reserve, cash) = if c.mint {
            let e = IBond::Mint::decode_log_data(event).ok()?;
            (
                e.token,
                e.user,
                e.receiver,
                e.amountMinted,
                e.reserveToken,
                e.reserveAmount,
            )
        } else {
            let e = IBond::Burn::decode_log_data(event).ok()?;
            (
                e.token,
                e.user,
                e.receiver,
                e.amountBurned,
                e.reserveToken,
                e.refundAmount,
            )
        };
        if (token, user, receiver, qty, reserve)
            != (
                c.token,
                c.protection.map_or(wallet, |p| p.module),
                wallet,
                c.qty,
                c.reserve,
            )
            || cash.is_zero()
        {
            return None;
        }
        if let Some(p) = c.protection {
            return self.protected_amounts(logs, p, cash);
        }
        let (debit, credit, from, to) = if c.mint {
            if cash > c.limit {
                return None;
            }
            (cash, qty, wallet, self.bond)
        } else {
            if cash < c.limit {
                return None;
            }
            (qty, cash, self.bond, wallet)
        };
        let moved = |a, f, t| fill::moved_logs(logs, a, f, t);
        let (issued_from, issued_to) = if c.mint {
            (Address::ZERO, wallet)
        } else {
            (wallet, Address::ZERO)
        };
        let paid = moved(reserve, Some(from), Some(to));
        let net = |a, account, inc| {
            let incoming = moved(a, None, Some(account));
            let outgoing = moved(a, Some(account), None);
            if inc {
                incoming.checked_sub(outgoing)
            } else {
                outgoing.checked_sub(incoming)
            }
        };
        (paid == cash
            && moved(token, Some(issued_from), Some(issued_to)) == qty
            && net(reserve, from, false) == Some(cash)
            && net(reserve, to, true) == Some(cash)
            && net(token, wallet, c.mint) == Some(qty))
        .then_some((debit, credit))
    }
}

impl Proof {
    fn protected_amounts(
        &self,
        logs: &[Log],
        p: super::mint_club_wire::Protection,
        cash: U256,
    ) -> Option<(U256, U256)> {
        use super::mint_club_wire::IBondFlow;
        let c = self.call;
        let w = self.payer;
        if self.bond != p.bond
            || p.module == w
            || p.module == p.bond
            || p.module.is_zero()
            || p.deadline.is_zero()
        {
            return None;
        }
        let mut rows = logs.iter().filter(|l| {
            l.address() == p.module && l.topic0() == Some(&IBondFlow::Settled::SIGNATURE_HASH)
        });
        let e = IBondFlow::Settled::decode_log_data(&rows.next()?.inner.data).ok()?;
        if rows.next().is_some()
            || (e.payer, e.token, e.reserve, e.mint, e.qty)
                != (w, c.token, c.reserve, c.mint, c.qty)
            || e.debit.is_zero()
            || e.credit.is_zero()
        {
            return None;
        }
        let moved = |a, f, t| fill::moved_logs(logs, a, f, t);
        let net = |a, who, inc| {
            let plus = moved(a, None, Some(who));
            let minus = moved(a, Some(who), None);
            if inc {
                plus.checked_sub(minus)
            } else {
                minus.checked_sub(plus)
            }
        };
        let (input, output) = c.pair();
        if net(input, w, false) != Some(e.debit)
            || net(output, w, true) != Some(e.credit)
            || net(input, p.module, true) != Some(U256::ZERO)
            || net(c.reserve, self.bond, c.mint) != Some(cash)
        {
            return None;
        }
        let correct = if c.mint {
            e.debit <= c.limit
                && e.credit == c.qty
                && cash <= c.limit
                && moved(c.token, Some(Address::ZERO), Some(w)) == c.qty
        } else {
            e.debit == c.qty
                && e.credit >= c.limit
                && cash >= c.limit
                && moved(c.token, Some(p.module), Some(Address::ZERO)) == c.qty
        };
        correct.then_some((e.debit, e.credit))
    }
}
#[cfg(test)]
#[path = "mint_club_fact_tests.rs"]
mod tests;
