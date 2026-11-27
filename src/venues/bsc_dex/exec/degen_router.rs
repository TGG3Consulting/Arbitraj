//! Прямой DEGENSwap Router02. Ставка администратора ограничена в проверенном runtime величиной 100/10000.
//! https://etherscan.io/address/0x4bf3e2287d4ced7796bfab364c0401dfce4a4f7f#code
use super::super::{
    ctx::Ctx,
    pools::{degenswap, exact_wallet_tokens, registry::Entry},
};
use crate::gateway::errors::GatewayResult;
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
sol! {interface Router{
    function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn,uint256 amountOutMin,address[] path,address to,uint256 deadline) external;
}}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    at: &Value,
) -> GatewayResult<Address> {
    let p = &e.pool;
    let (router, ()) = tokio::try_join!(
        degenswap::router(
            ctx,
            e.family,
            p.reference.address(),
            (p.token0, p.token1),
            at
        ),
        exact_wallet_tokens::check_input(ctx, e.family.id, input, at)
    )?;
    Ok(router)
}
pub(super) fn gross(m: U256) -> Option<U256> {
    m.checked_add(m.checked_sub(U256::ONE)? / U256::from(99))
}
pub(super) fn data(p: &super::safeswap_wallet::Proof) -> Bytes {
    let Some(cap) = gross(p.minimum) else {
        return Bytes::new();
    };
    Router::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
        amountIn: p.budget,
        amountOutMin: cap,
        path: vec![p.input, p.output],
        to: p.payer,
        deadline: p.deadline,
    }
    .abi_encode()
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_amount_is_the_least_that_still_guarantees_the_wallet_floor() {
        for n in [
            1u64, 2, 98, 99, 100, 101, 197, 198, 199, 200, 9999, 10000, 123456789,
        ] {
            let m = U256::from(n);
            let g = gross(m).unwrap();
            let prior = g - U256::ONE;
            assert!(prior - prior / U256::from(100) < m);
            for rate in [0, 1, 50, 99, 100] {
                assert!(g - g * U256::from(rate) / U256::from(10000) >= m);
            }
        }
        assert_eq!(gross(U256::ZERO), None);
        assert_eq!(gross(U256::MAX), None);
    }
}
