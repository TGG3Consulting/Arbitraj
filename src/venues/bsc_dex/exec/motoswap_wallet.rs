//! Recover the signed Motoswap order and terminal FeeRouter / module payment after a restart.
use super::{carbon_binding::Code, fill, signed_call};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        check::simulate::SimLog, ctx::Ctx, pools::solidly_classic_factory,
        quote::v2_wallet_sim::Order, send::track::Receipt,
    },
};
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};

#[allow(clippy::too_many_arguments)]
pub(in crate::venues::bsc_dex) mod abi {
    alloy::sol! {
        interface IModule {
            function execute(address pool,address input,address output,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
            function pathVersion() external view returns(uint256);
            function executePath(address[] pools,address[] path,uint256 amount,uint256 minimum,uint256 deadline) external returns(uint256);
            event PathPaid(bytes32 indexed route,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
            function factory() external view returns(address);
            function router() external view returns(address);
            event Paid(address indexed pool,address indexed payer,address indexed recipient,address input,address output,uint256 spent,uint256 received);
        }
        interface IFee {
            function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn,uint256 amountOutMin,address[] path,address to,uint256 deadline) external returns(uint256);
            event Swap(address indexed user,address indexed tokenIn,address indexed tokenOut,uint256 amountIn,uint256 amountOut,address feeToken,uint256 feeAmount);
        }
        interface IPair {event Swap(address indexed sender,uint256 amount0In,uint256 amount1In,uint256 amount0Out,uint256 amount1Out,address indexed to);}
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Terms {
    pub pool: Address,
    pub target: Address,
    pub fee: Address,
    pub core: Address,
    pub fee_code: Code,
    pub module_hash: Option<B256>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub pools: Vec<Address>,
    pub path: Vec<Address>,
}
impl Route {
    pub(in crate::venues::bsc_dex) fn proof(
        &self,
        t: Terms,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        let mut p = t.proof(o, minimum, deadline);
        p.route = Some(self.clone());
        p
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
    pub terms: Terms,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub maximum: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Terms {
    pub(in crate::venues::bsc_dex) fn proof(
        self,
        o: &Order,
        minimum: U256,
        deadline: U256,
    ) -> Proof {
        Proof {
            route: None,
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
fn unique<E: SolEvent>(logs: &[Log], addr: Address) -> Option<E> {
    let mut candidates = logs
        .iter()
        .filter(|l| l.address() == addr && l.topic0() == Some(&E::SIGNATURE_HASH));
    let event = E::decode_log_data(&candidates.next()?.inner.data).ok()?;
    candidates.next().is_none().then_some(event)
}
impl Proof {
    pub fn data(&self) -> Bytes {
        if let Some(route) = &self.route {
            return if self.terms.module_hash.is_some() {
                abi::IModule::executePathCall {
                    pools: route.pools.clone(),
                    path: route.path.clone(),
                    amount: self.maximum,
                    minimum: self.minimum,
                    deadline: self.deadline,
                }
                .abi_encode()
                .into()
            } else {
                abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                    amountIn: self.maximum,
                    amountOutMin: self.minimum,
                    path: route.path.clone(),
                    to: self.payer,
                    deadline: self.deadline,
                }
                .abi_encode()
                .into()
            };
        }
        if self.terms.module_hash.is_some() {
            abi::IModule::executeCall {
                pool: self.terms.pool,
                input: self.input,
                output: self.output,
                amount: self.maximum,
                minimum: self.minimum,
                deadline: self.deadline,
            }
            .abi_encode()
            .into()
        } else {
            abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                amountIn: self.maximum,
                amountOutMin: self.minimum,
                path: vec![self.input, self.output],
                to: self.payer,
                deadline: self.deadline,
            }
            .abi_encode()
            .into()
        }
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.terms.target,
            U256::ZERO,
            self.data(),
        )
        .await?;
        self.terms
            .fee_code
            .at_transaction(ctx, self.terms.fee, hash, r, None)
            .await?;
        if let Some(expected) = self.terms.module_hash {
            let block = r.block_hash.ok_or_else(|| {
                ctx.err(ErrorCode::OutcomeUnknown, "Motoswap: нет блока включения")
            })?;
            solidly_classic_factory::plain(
                ctx,
                self.terms.target,
                expected,
                &serde_json::json!({"blockHash":block,"requireCanonical":true}),
            )
            .await?;
        }
        crate::venues::bsc_dex::send::canonical_receipt::verify(ctx, hash, r).await
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::Read;
    fn archive(bytes: &[u8]) -> Value {
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut text)
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }
    #[test]
    fn restores_the_original_and_guarded_payments_from_signed_calldata() {
        let original = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"
        ));
        let guarded = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_wallets.json.gz"
        ));
        let bk = &original["directory"];
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let hash = |v: &Value| v.as_str().unwrap().parse::<B256>().unwrap();
        let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let fee = addr(&bk["roles"]["fee_router"]);
        let core = addr(&bk["roles"]["core"]);
        let code = Code {
            shell: hash(&bk["code"]["fee_router"]),
            implementation: addr(&bk["roles"]["fee_impl"]),
            runtime: hash(&bk["code"]["fee_impl"]),
        };
        let payer = addr(&original["wallet"]);
        let native = original["trials"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (&r["paid"], addr(&r["pool"]), false));
        let wallet = guarded["payments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["mode"] == "wallet")
            .map(|r| (r, addr(&r["pool"]), true));
        let mut count = 0;
        for (r, pool, protected) in native.chain(wallet) {
            let raw: Bytes = r["calldata"].as_str().unwrap().parse().unwrap();
            let (input, output, maximum, minimum, deadline) = if protected {
                let c = abi::IModule::executeCall::abi_decode_validate(&raw).unwrap();
                (c.input, c.output, c.amount, c.minimum, c.deadline)
            } else {
                let c=abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode_validate(&raw).unwrap();
                (c.path[0], c.path[1], c.amountIn, c.amountOutMin, c.deadline)
            };
            let terms = Terms {
                pool,
                fee,
                core,
                fee_code: code,
                target: if protected {
                    addr(&guarded["module"])
                } else {
                    fee
                },
                module_hash: protected.then(|| hash(&guarded["manifest"]["runtime"])),
            };
            let proof = Proof {
                route: None,
                terms,
                payer,
                input,
                output,
                maximum,
                minimum,
                deadline,
            };
            let saved = serde_json::to_vec(&proof).unwrap();
            let mut restored: Proof = serde_json::from_slice(&saved).unwrap();
            let logs: Vec<SimLog> = serde_json::from_value(r["logs"].clone()).unwrap();
            assert_eq!(restored.data(), raw);
            assert_eq!(
                restored.measured(&logs),
                Some((num(&r["spent"]), num(&r["received"])))
            );
            restored.minimum += U256::ONE;
            assert!(restored.measured(&logs).is_none());
            count += 1;
        }
        assert_eq!(count, 32);
    }
    #[test]
    fn recovers_both_physical_pools_and_the_signed_terminal_minimum() {
        let book = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"
        ));
        let data = archive(include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_paths.json.gz"
        ));
        let dir = &book["directory"];
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let hash = |v: &Value| v.as_str().unwrap().parse::<B256>().unwrap();
        let fee = addr(&dir["roles"]["fee_router"]);
        let payer = addr(&data["owner"]);
        let mut count = 0;
        for r in data["payments"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["mode"] != "v5_steps")
        {
            let guarded = r["mode"] == "guarded";
            let raw: Bytes = r["calldata"].as_str().unwrap().parse().unwrap();
            let (path, amount, minimum, deadline) = if guarded {
                let c = abi::IModule::executePathCall::abi_decode_validate(&raw).unwrap();
                (c.path, c.amount, c.minimum, c.deadline)
            } else {
                let c=abi::IFee::swapExactTokensForTokensSupportingFeeOnTransferTokensCall::abi_decode_validate(&raw).unwrap();
                (c.path, c.amountIn, c.amountOutMin, c.deadline)
            };
            let pools: Vec<_> = r["markets"].as_array().unwrap().iter().map(addr).collect();
            let terms = Terms {
                pool: pools[0],
                target: if guarded { addr(&data["module"]) } else { fee },
                fee,
                core: addr(&dir["roles"]["core"]),
                fee_code: Code {
                    shell: hash(&dir["code"]["fee_router"]),
                    implementation: addr(&dir["roles"]["fee_impl"]),
                    runtime: hash(&dir["code"]["fee_impl"]),
                },
                module_hash: guarded.then(|| hash(&data["runtime"])),
            };
            let p = Proof {
                terms,
                payer,
                input: path[0],
                output: *path.last().unwrap(),
                maximum: amount,
                minimum,
                deadline,
                route: Some(Route { pools, path }),
            };
            let mut saved: Proof =
                serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
            let logs: Vec<SimLog> = serde_json::from_value(r["logs"].clone()).unwrap();
            assert_eq!(saved.data(), raw);
            assert_eq!(
                saved.measured(&logs),
                Some((num(&r["spent"]), num(&r["received"])))
            );
            saved.minimum += U256::ONE;
            assert_eq!(saved.measured(&logs), None);
            saved.minimum -= U256::ONE;
            saved.route.as_mut().unwrap().pools.swap(0, 1);
            assert_eq!(saved.measured(&logs), None);
            count += 1;
        }
        assert_eq!(count, 12);
    }
}
