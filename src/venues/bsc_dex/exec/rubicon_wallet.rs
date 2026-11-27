//! Wallet-bound Rubicon Classic maker order execution and reconciliation.
use super::{carbon_binding::Code, fill, signed_call};
use crate::{
    gateway::errors::GatewayResult,
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, quote::v2_wallet_sim::Order, send::track::Receipt,
    },
};
use alloy::{
    rpc::types::Log,
    sol,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

sol! { interface IBookWallet {
    struct Take {uint256 id; uint128 quantity;}
    function market() external view returns(address);
    function marketCodeHash() external view returns(bytes32);
    function quote(address source,address target,Take[] actions,uint256 maximum) external view returns(uint256 spent,uint256 nominal);
    function execute(address source,address target,Take[] actions,uint256 maximum,uint256 minimum,uint256 deadline) external returns(uint256 spent,uint256 received);
    event Settled(address indexed payer,address indexed source,address indexed target,bytes32 actionsHash,uint256 spent,uint256 received);
    event emitTake(bytes32 indexed id,bytes32 indexed pair,address indexed maker,address taker,address pay_gem,address buy_gem,uint128 take_amt,uint128 give_amt);
}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terms {
    pub market: Address,
    pub target: Address,
    pub code: Code,
    pub wallet_hash: B256,
    pub actions: Vec<(U256, u128)>,
    pub desired: Option<U256>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(crate) fn proof(&self, o: &Order, minimum: U256, deadline: U256) -> Proof {
        Proof {
            terms: self.clone(),
            payer: o.payer,
            input: o.input,
            output: o.output,
            amount: o.amount,
            minimum,
            deadline,
        }
    }
}
impl Proof {
    pub(crate) fn data(&self) -> Bytes {
        IBookWallet::executeCall {
            source: self.input,
            target: self.output,
            actions: self
                .terms
                .actions
                .iter()
                .map(|&(id, quantity)| IBookWallet::Take { id, quantity })
                .collect(),
            maximum: self.amount,
            minimum: self.minimum,
            deadline: self.deadline,
        }
        .abi_encode()
        .into()
    }
    pub(super) async fn included(&self, ctx: &Ctx, h: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            h,
            r,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        self.terms
            .code
            .at_transaction(ctx, self.terms.market, h, r, None)
            .await?;
        let at = serde_json::json!({"blockHash":r.block_hash.ok_or_else(||ctx.err(crate::gateway::errors::ErrorCode::OutcomeUnknown,"Rubicon Classic: нет хеша блока"))?,"requireCanonical":true});
        crate::venues::bsc_dex::pools::solidly_classic_factory::plain(
            ctx,
            self.terms.target,
            self.terms.wallet_hash,
            &at,
        )
        .await?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, h, r).await
    }
    pub(crate) fn measured(&self, rows: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(rows)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        owner: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = &self.terms;
        if (owner, input, output) != (self.payer, self.input, self.output)
            || input == output
            || t.actions.is_empty()
        {
            return None;
        }
        let mut settled = logs.iter().filter(|l| {
            l.address() == t.target && l.topic0() == Some(&IBookWallet::Settled::SIGNATURE_HASH)
        });
        let s = IBookWallet::Settled::decode_log_data(&settled.next()?.inner.data).ok()?;
        if settled.next().is_some()
            || (s.payer, s.source, s.target) != (owner, input, output)
            || s.actionsHash != keccak256(t.actions.abi_encode())
            || s.spent.is_zero()
            || s.spent > self.amount
            || s.received.is_zero()
            || s.received < self.minimum
        {
            return None;
        }
        let pair = keccak256([output.as_slice(), input.as_slice()].concat());
        let mut fills = logs.iter().filter(|l| {
            l.address() == t.market && l.topic0() == Some(&IBookWallet::emitTake::SIGNATURE_HASH)
        });
        for &(id, quantity) in &t.actions {
            let f = IBookWallet::emitTake::decode_log_data(&fills.next()?.inner.data).ok()?;
            if (f.taker, f.pay_gem, f.buy_gem, f.pair) != (t.target, output, input, pair)
                || U256::from_be_bytes(f.id.0) != id
                || f.take_amt != quantity
                || f.give_amt == 0
            {
                return None;
            }
        }
        if fills.next().is_some() {
            return None;
        }
        let moved = |tok, from, to| fill::moved_logs(logs, tok, from, to);
        let debit = moved(input, Some(owner), None).checked_sub(moved(input, None, Some(owner)))?;
        let credit =
            moved(output, None, Some(owner)).checked_sub(moved(output, Some(owner), None))?;
        ((debit, credit) == (s.spent, s.received)
            && moved(output, Some(t.market), Some(t.target)) >= credit
            && moved(input, Some(owner), Some(t.target)) == self.amount
            && moved(output, Some(t.target), Some(owner)) == credit)
            .then_some((debit, credit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn six_paid_orders_require_complete_wallet_and_market_evidence() {
        let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
            &include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/rubicon_classic_base.json.gz"
            )[..],
        ))
        .unwrap();
        let addr = |v: &Value| -> Address { v.as_str().unwrap().parse().unwrap() };
        let raw = |v: &Value| -> U256 { v.as_str().unwrap().parse().unwrap() };
        let role = &archive["market"];
        let shell = role["proxy"].as_str().unwrap();
        let implementation = role["implementation"].as_str().unwrap();
        let code = Code {
            shell: role["hashes"][shell].as_str().unwrap().parse().unwrap(),
            implementation: implementation.parse().unwrap(),
            runtime: role["hashes"][implementation]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        };
        let controls = archive["controls"].as_array().unwrap();
        assert_eq!(controls.len(), 6);
        for r in controls {
            let t = Terms {
                market: shell.parse().unwrap(),
                target: addr(&archive["module"]),
                code,
                wallet_hash: archive["module_hash"].as_str().unwrap().parse().unwrap(),
                actions: r["actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| {
                        (
                            U256::from(a[0].as_u64().unwrap()),
                            a[1].as_str().unwrap().parse().unwrap(),
                        )
                    })
                    .collect(),
                desired: None,
            };
            let p = Proof {
                terms: t,
                payer: addr(&r["wallet"]),
                input: addr(&r["source"]),
                output: addr(&r["target"]),
                amount: raw(&r["budget"]),
                minimum: raw(&r["received"]),
                deadline: U256::from(r["deadline"].as_u64().unwrap()),
            };
            let mut logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
            assert_eq!(
                p.amounts(&logs, p.payer, p.input, p.output),
                Some((raw(&r["spent"]), raw(&r["received"])))
            );
            logs.retain(|l| l.topic0() != Some(&IBookWallet::emitTake::SIGNATURE_HASH));
            assert!(p.amounts(&logs, p.payer, p.input, p.output).is_none());
        }
    }
}
