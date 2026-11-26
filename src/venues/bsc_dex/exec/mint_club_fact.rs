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
                && t.to == Some(self.bond)
                && t.value.is_zero()
                && t.input == self.call.data(self.payer)
        }) {
            Ok(())
        } else {
            Err(fail())
        }
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
        if (token, user, receiver, qty, reserve) != (c.token, wallet, wallet, c.qty, c.reserve)
            || cash.is_zero()
        {
            return None;
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

#[cfg(test)]
#[path = "mint_club_fact_tests.rs"]
mod tests;
