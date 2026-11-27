//! Повтор проверки выплаты DefiPlaza через неизменяемые роли модуля, физический пул и подписанную транзакцию.
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

mod abi {
    alloy::sol! { interface IModule {
        function execute(address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external payable returns(uint256);
        event Filled(address indexed payer,address indexed recipient,address indexed input,address output,uint256 spent,uint256 received);
    } }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub pool: Address,
    pub target: Address,
    pub wrapped: Address,
    pub native: bool,
    pub stable: bool,
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
            input: t.asset(self.input),
            output: t.asset(self.output),
            amount: self.maximum,
            minimum: self.minimum,
            deadline: self.deadline,
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
                "DefiPlaza: отсутствует блок квитанции",
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
        if (payer, input, output) != (self.payer, self.input, self.output) || self.minimum.is_zero()
        {
            return None;
        }
        let paid = unique::<abi::IModule::Filled>(logs, t.target)?;
        if (paid.payer, paid.recipient, paid.input, paid.output)
            != (payer, payer, t.asset(input), t.asset(output))
            || paid.spent != self.maximum
            || paid.received < self.minimum
        {
            return None;
        }
        let physical = |a| {
            if !t.stable && a == t.wrapped {
                Address::ZERO
            } else {
                a
            }
        };
        let (sender, src, dst, spent, gross) = if t.stable {
            let evt =
                unique::<crate::venues::bsc_dex::pools::defiplaza::IPlaza::Swap>(logs, t.pool)?;
            if evt.destination != t.target {
                return None;
            }
            (
                evt.sender,
                evt.inputToken,
                evt.outputToken,
                evt.inputAmount,
                evt.outputAmount,
            )
        } else {
            let evt =
                unique::<crate::venues::bsc_dex::pools::defiplaza::IPlaza::Swapped>(logs, t.pool)?;
            (
                evt.sender,
                evt.inputToken,
                evt.outputToken,
                evt.inputAmount,
                evt.outputAmount,
            )
        };
        if (sender, src, dst) != (t.target, physical(input), physical(output))
            || spent.is_zero()
            || gross < paid.received
            || (!t.stable && spent != paid.spent)
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
    fn canonical_wire_and_terminal_transfers_match_both_generations_and_currencies() {
        let wrapped = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
            .parse()
            .unwrap();
        for (bytes,stable) in [(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/defiplaza_multi_protected_ethereum.json.gz").as_slice(),false),
            (include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/defiplaza_stable_protected_ethereum.json.gz").as_slice(),true)]{
            let book:Value=serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap();
            let addr=|v:&Value|v.as_str().unwrap().parse::<Address>().unwrap();
            let payer=addr(&book["payer"]);
            let mut count=0;
            for row in book["fills"].as_array().unwrap().iter().filter(|r|r["mode"]!="v5"){
                let data:Bytes=row["action"]["data"].as_str().unwrap().parse().unwrap();
                let wire=abi::IModule::executeCall::abi_decode_validate(&data).unwrap();
                let normalized=|a:Address|if a.is_zero(){wrapped}else{a};
                let proof=Proof{terms:Terms{pool:addr(&book["pool"]),target:addr(&book["module"]),wrapped,native:row["mode"]=="native",stable,
                    module_hash:book["runtime"].as_str().unwrap().parse().unwrap()},payer,input:normalized(wire.input),output:normalized(wire.output),
                    maximum:wire.amount,minimum:wire.minimum,deadline:wire.deadline};
                assert_eq!(proof.data(),data);
                let logs:Vec<Log>=serde_json::from_value(row["logs"].clone()).unwrap();
                assert_eq!(proof.amounts(&logs,payer,proof.input,proof.output),Some((proof.maximum,proof.minimum)));
                assert!(proof.amounts(&logs,Address::ZERO,proof.input,proof.output).is_none());
                let mut duplicate=logs.clone();duplicate.extend_from_slice(&logs);
                assert!(proof.amounts(&duplicate,payer,proof.input,proof.output).is_none());
                assert!(row["zero_min"].is_object() && row["plus_one"].is_object() && row["expiry"].is_object());
                count+=1;
            }
            assert_eq!(count,12);
        }
    }
}
