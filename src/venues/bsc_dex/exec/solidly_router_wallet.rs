//! Signed single-pair Router01 trade and receipt for exact-transfer Ethereum assets.
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    registered_v3_wallet::RouterMarket,
    white_direct,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        pools::solidly_proxy,
        quote::{v2_wallet_sim, white_state},
    },
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

sol! {interface Router {
    function swapExactTokensForTokensSimple(uint256 amountIn,uint256 amountOutMin,address tokenFrom,address tokenTo,
        bool stable,address to,uint256 deadline) external returns(uint256[] amounts);
}}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Market {
    pub pool: Address,
    pub stable: bool,
}
impl RouterMarket for Market {
    fn pool(self) -> Address {
        self.pool
    }
    fn event(self) -> (&'static str, usize) {
        ("Swap(address,uint256,uint256,uint256,uint256,address)", 128)
    }
    fn matches(self, data: &[u8], i: usize, spent: U256, received: U256, budget: U256) -> bool {
        let word = |j: usize| U256::from_be_slice(&data[j * 32..(j + 1) * 32]);
        spent == budget
            && word(i) == spent
            && word(1 - i).is_zero()
            && word(2 + i).is_zero()
            && word(3 - i) == received
    }
    fn order(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        wallet: Address,
        deadline: U256,
    ) -> Bytes {
        Router::swapExactTokensForTokensSimpleCall {
            amountIn: amount,
            amountOutMin: minimum,
            tokenFrom: input,
            tokenTo: output,
            stable: self.stable,
            to: wallet,
            deadline,
        }
        .abi_encode()
        .into()
    }
}
pub type Proof = super::registered_v3_wallet::Proof<Market>;

pub(super) async fn plan(
    ctx: &Ctx,
    s: &Step,
    wallet: Address,
    height: u64,
    selected: Option<Address>,
) -> GatewayResult<Plan> {
    let fail = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Solidly Router01: не совпали кошелёк, пара или исходный адрес оплаты",
        )
    };
    let generation = ctx.settings.generation();
    let e = &s.entry;
    let o = &s.order;
    let head = white_state::head(ctx, &format!("{height:#x}")).await?;
    let target = solidly_proxy::router(ctx, e, o.token_in, o.token_out, &head.selector()).await?;
    if selected != Some(target) || !s.from_wallet || !s.to_wallet {
        return Err(fail());
    }
    let crate::core::types::PoolState::Solidly { stable, .. } = &e.pool.state else {
        return Err(fail());
    };
    let market = Market {
        pool: e.pool.reference.address(),
        stable: *stable,
    };
    let now = head.timestamp.to::<u64>();
    let proof = Proof {
        market,
        target,
        payer: wallet,
        input: o.token_in,
        output: o.token_out,
        budget: o.amount_in,
        minimum: o.min_out,
        deadline: U256::from(
            now.checked_add(ctx.int("dex.deadline_s")?)
                .ok_or_else(fail)?,
        ),
    };
    let order = v2_wallet_sim::Order {
        payer: wallet,
        target,
        input: o.token_in,
        output: o.token_out,
        amount: o.amount_in,
        minimum: o.min_out,
        wire: v2_wallet_sim::Wire::SolidlyRouter(market),
    };
    v2_wallet_sim::measure(ctx, head, order, true).await?;
    let mut p = white_direct::build(ctx, s, wallet, target, now, false, generation).await?;
    p.set_direct_market(DirectCall::SolidlyRouter(Arc::new(proof)));
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::check::simulate::SimLog;
    use flate2::read::GzDecoder;
    use serde_json::Value;

    #[test]
    fn saved_call_keeps_the_exact_pool_payer_volume_and_floor() {
        let bytes = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/solidly_v2_ethereum_wallets.json.gz"
        );
        let archive: Value = serde_json::from_reader(GzDecoder::new(&bytes[..])).unwrap();
        let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
        let mut verified = 0;
        for m in archive["markets"].as_array().unwrap() {
            let market = Market {
                pool: addr(&m["pool"]),
                stable: m["stable"].as_bool().unwrap(),
            };
            for c in m["cases"].as_array().unwrap() {
                let payer = addr(&c["owner"]);
                let raw = &c["router"];
                for (side, idx, input, output, budget, minimum) in [
                    (
                        "buy",
                        4,
                        &m["token_in"],
                        &m["token_out"],
                        &c["amount"],
                        &c["bought"],
                    ),
                    (
                        "sell",
                        9,
                        &m["token_out"],
                        &m["token_in"],
                        &c["bought"],
                        &c["sold"],
                    ),
                ] {
                    let wire = &raw["calls"][idx];
                    let calldata: Bytes = wire["data"].as_str().unwrap().parse().unwrap();
                    let args =
                        Router::swapExactTokensForTokensSimpleCall::abi_decode_validate(&calldata)
                            .unwrap();
                    let proof = Proof {
                        market,
                        target: addr(&wire["to"]),
                        payer,
                        input: addr(input),
                        output: addr(output),
                        budget: budget.as_str().unwrap().parse().unwrap(),
                        minimum: minimum.as_str().unwrap().parse().unwrap(),
                        deadline: args.deadline,
                    };
                    let mut recovered: Proof =
                        serde_json::from_value(serde_json::to_value(&proof).unwrap()).unwrap();
                    assert_eq!(recovered.data(), calldata);
                    let logs: Vec<SimLog> =
                        serde_json::from_value(raw["results"][side]["logs"].clone()).unwrap();
                    assert_eq!(
                        recovered.measured(&logs),
                        Some((proof.budget, proof.minimum))
                    );
                    recovered.minimum += U256::ONE;
                    assert_eq!(recovered.measured(&logs), None);
                    recovered.minimum = proof.minimum;
                    recovered.budget += U256::ONE;
                    assert_eq!(recovered.measured(&logs), None);
                    recovered.budget = proof.budget;
                    recovered.target = Address::ZERO;
                    assert_eq!(recovered.measured(&logs), None);
                    verified += 1;
                }
            }
        }
        assert!(verified >= 16);
    }
}
