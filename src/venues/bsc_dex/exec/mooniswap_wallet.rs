//! Повтор проверки выплаты Mooniswap через неизменяемые роли модуля, физический пул и подписанную транзакцию.
use super::{fill, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory,
        quote::v2_wallet_sim::Order, send::track::Receipt,
    },
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        interface IModule {
            struct Order { bytes32 market; address input; address output; uint256 amount; uint256 minimum; uint256 deadline; }
            function execute(Order order) external payable returns(uint256);
            function factory() external view returns(address);
            function wrapped() external view returns(address);
            function hasSwapFor() external view returns(bool);
            event Paid(address indexed payer,address indexed recipient,address indexed pool,address input,address output,uint256 spent,uint256 received);
        }
        interface IOriginal {event Swapped(address indexed account,address indexed src,address indexed dst,uint256 amount,uint256 result,uint256 srcBalance,uint256 dstBalance,uint256 totalSupply,address referral);}
        interface ISuccessor {event Swapped(address indexed sender,address indexed receiver,address indexed src,address dst,uint256 amount,uint256 result,uint256 srcAdditionBalance,uint256 dstRemovalBalance,address referral);}
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub market: B256,
    pub target: Address,
    pub wrapped: Address,
    pub native: bool,
    pub successor: bool,
    pub module_hash: B256,
}
impl Terms {
    pub fn asset(&self, token: Address) -> Address {
        if self.native && token == self.wrapped {
            Address::ZERO
        } else {
            token
        }
    }
    pub fn value(&self, input: Address, amount: U256) -> U256 {
        if self.asset(input).is_zero() {
            amount
        } else {
            U256::ZERO
        }
    }
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            terms: self,
            payer: o.payer,
            input: o.input,
            output: o.output,
            maximum: o.amount,
            minimum,
            deadline,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub maximum: U256,
    pub minimum: U256,
    pub deadline: U256,
}
fn unique<E: SolEvent>(logs: &[Log], at: Address) -> Option<E> {
    let mut rows = logs
        .iter()
        .filter(|l| l.address() == at && l.topic0() == Some(&E::SIGNATURE_HASH));
    let evt = E::decode_log_data(&rows.next()?.inner.data).ok()?;
    rows.next().is_none().then_some(evt)
}
impl Proof {
    pub fn data(&self) -> Bytes {
        let t = self.terms;
        abi::IModule::executeCall {
            order: abi::IModule::Order {
                market: t.market,
                input: t.asset(self.input),
                output: t.asset(self.output),
                amount: self.maximum,
                minimum: self.minimum,
                deadline: self.deadline,
            },
        }
        .abi_encode()
        .into()
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        let t = self.terms;
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            t.target,
            t.value(self.input, self.maximum),
            self.data(),
        )
        .await?;
        let block = r.block_hash.ok_or_else(|| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "Mooniswap: отсутствует блок квитанции",
            )
        })?;
        solidly_classic_factory::plain(
            ctx,
            t.target,
            t.module_hash,
            &serde_json::json!({"blockHash":block,"requireCanonical":true}),
        )
        .await?;
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, hash, r).await
    }
    pub fn measured(&self, logs: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
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
        let bits = t.market.as_slice();
        if (payer, input, output) != (self.payer, self.input, self.output)
            || bits[..11] != [0; 11]
            || bits[11] > 2
            || input == output
            || self.minimum.is_zero()
        {
            return None;
        }
        let pool = Address::from_slice(&bits[12..]);
        let physical = |asset: Address, flag| {
            if bits[11] == flag {
                (asset.is_zero() || asset == t.wrapped).then_some(Address::ZERO)
            } else {
                Some(if asset.is_zero() { t.wrapped } else { asset })
            }
        };
        let src = physical(input, 1)?;
        let dst = physical(output, 2)?;
        let paid = unique::<abi::IModule::Paid>(logs, t.target)?;
        if (
            paid.payer,
            paid.recipient,
            paid.pool,
            paid.input,
            paid.output,
        ) != (payer, payer, pool, t.asset(input), t.asset(output))
            || paid.spent != self.maximum
            || paid.received < self.minimum
        {
            return None;
        }
        let (source, output_token, from, amount, result, referral) = if t.successor {
            let x = unique::<abi::ISuccessor::Swapped>(logs, pool)?;
            let recipient = if dst == paid.output { payer } else { t.target };
            if x.receiver != recipient {
                return None;
            }
            (x.src, x.dst, x.sender, x.amount, x.result, x.referral)
        } else {
            let x = unique::<abi::IOriginal::Swapped>(logs, pool)?;
            (x.src, x.dst, x.account, x.amount, x.result, x.referral)
        };
        if (source, output_token, from) != (src, dst, t.target)
            || !referral.is_zero()
            || amount.is_zero()
            || amount > paid.spent
            || result < paid.received
        {
            return None;
        }
        let inbound = |asset| fill::moved_logs(logs, asset, None, Some(payer));
        let outbound = |asset| fill::moved_logs(logs, asset, Some(payer), None);
        if !paid.input.is_zero() && outbound(input).checked_sub(inbound(input))? != paid.spent {
            return None;
        }
        if !paid.output.is_zero() && inbound(output).checked_sub(outbound(output))? != paid.received
        {
            return None;
        }
        Some((paid.spent, paid.received))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn paid_wallets_recover_the_signed_amount_and_real_currency_in_both_generations() {
        let original = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mooniswap_original_wallets.json.gz"
        );
        let successor = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mooniswap_successor_wallets.json.gz"
        );
        let wrapped = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
            .parse::<Address>()
            .unwrap();
        for (bytes, next, wallet_count, v5_count) in [
            (original.as_slice(), false, 40, 24),
            (successor.as_slice(), true, 36, 24),
        ] {
            let book: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
            let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
            let units = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
            let owner = addr(&book["owner"]);
            let mut counts = [0, 0];
            for row in book["cases"].as_array().unwrap() {
                let maximum = units(&row["amount"]);
                let minimum = units(&row["received"]);
                let before = row["before"].as_array().unwrap();
                let after = row["after"].as_array().unwrap();
                assert_eq!(units(&before[0]) - units(&after[0]), maximum);
                assert_eq!(units(&after[1]) - units(&before[1]), minimum);
                assert_eq!(&before[4..], &after[4..]);
                assert!(row["high"].is_object() && row["expired"].is_object());
                if row["mode"] == "v5" {
                    counts[1] += 1;
                    continue;
                }
                let raw: Bytes = row["trade"]["data"].as_str().unwrap().parse().unwrap();
                let order = abi::IModule::executeCall::abi_decode_validate(&raw)
                    .unwrap()
                    .order;
                let physical = [addr(&row["input"]), addr(&row["output"])];
                let normalized = physical.map(|a| if a.is_zero() { wrapped } else { a });
                // ETH and the WETH token are distinct settleable currencies even in their own pool.
                let alias = normalized[0] == normalized[1];
                let [input, output] = if alias { physical } else { normalized };
                let terms = Terms {
                    market: order.market,
                    target: addr(&book["module"]),
                    wrapped,
                    native: !alias && physical.contains(&Address::ZERO),
                    successor: next,
                    module_hash: book["module_hash"].as_str().unwrap().parse().unwrap(),
                };
                let mut proof = Proof {
                    terms,
                    payer: owner,
                    input,
                    output,
                    maximum,
                    minimum,
                    deadline: order.deadline,
                };
                let encoded = serde_json::to_vec(&proof).unwrap();
                let restored: Proof = serde_json::from_slice(&encoded).unwrap();
                assert_eq!(restored.data(), raw);
                let logs: Vec<SimLog> = serde_json::from_value(row["logs"].clone()).unwrap();
                assert_eq!(restored.measured(&logs), Some((maximum, minimum)));
                proof.minimum += U256::ONE;
                assert_eq!(proof.measured(&logs), None);
                proof.minimum = minimum;
                proof.payer = Address::ZERO;
                assert_eq!(proof.measured(&logs), None);
                counts[0] += 1;
            }
            assert_eq!(counts, [wallet_count, v5_count]);
        }
    }
}
