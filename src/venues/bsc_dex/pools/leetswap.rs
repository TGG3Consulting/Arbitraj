//! LeetSwap V2 сверяет получателя комиссии, паузу и фактическое обеспечение каждой пары.
//! V5 вызывает getAmountOut от своего адреса, а пара определяет комиссию по получателю swap.
//! Пока фабрика использует общую ставку, эти вызовы эквивалентны для всех адресов.
use super::super::{ctx::Ctx, node::abi::IERC20};
use super::{abi_solidly::ISolidlyPool as P, families::Family, nami_batch as batch};
use crate::core::types::{FeeRule, PoolState, SolidlyFee, SolidlyGate};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, U256};

sol! {
    interface IFactory {
        function isPaused() external view returns (bool);
        function tradingFeesOracle() external view returns (address);
        function tradingFees(address pair, address to) external view returns (uint256);
    }
}

/// Пара из данной фабрики поддерживает адрес самого пула, V5 и конечный кошелёк на одинаковых условиях.
fn eligible(paused: bool, oracle: Address, reserves: [U256; 2], balances: [U256; 2]) -> bool {
    !paused && oracle.is_zero() && reserves == balances && reserves.iter().all(|r| !r.is_zero())
}

pub(crate) async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    tag: &str,
) -> GatewayResult<PoolState> {
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let block = super::elfomo::block(ctx, tag).await?;
    let quote = IFactory::tradingFeesCall {
        pair: pool,
        to: Address::ZERO,
    };
    let [meta, pause, oracle, fee] = batch::read(
        ctx,
        [
            batch::view(&block, pool, P::metadataCall {}),
            batch::view(&block, factory, IFactory::isPausedCall {}),
            batch::view(&block, factory, IFactory::tradingFeesOracleCall {}),
            batch::view(&block, factory, quote.clone()),
        ],
    )
    .await?;
    let state = batch::decode(&meta, P::metadataCall {})?;
    let balance = IERC20::balanceOfCall { owner: pool };
    let [b0, b1] = batch::read(
        ctx,
        [
            batch::view(&block, state.t0, balance.clone()),
            batch::view(&block, state.t1, balance.clone()),
        ],
    )
    .await?;
    let open = eligible(
        batch::decode(&pause, IFactory::isPausedCall {})?,
        batch::decode(&oracle, IFactory::tradingFeesOracleCall {})?,
        [state.r0, state.r1],
        [
            batch::decode(&b0, balance.clone())?,
            batch::decode(&b1, balance)?,
        ],
    );
    let fee = batch::decode(&fee, quote)?;
    if fee > U256::from(100) {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "LeetSwap: ставка превышает максимум фабрики",
        ));
    }
    Ok(PoolState::Solidly {
        reserve0: state.r0,
        reserve1: state.r1,
        stable: state.st,
        decimals0: state.dec0,
        decimals1: state.dec1,
        fee: SolidlyFee {
            value: fee.to::<u32>(),
            rule: FeeRule::Numerator {
                denominator: 10_000,
            },
        },
        fee1: None,
        gate: SolidlyGate {
            paused: !open,
            max_gas_price: None,
        },
    })
}

/// Сверяет на одном каноническом состоянии точные резервы, стоимость и ограничения до отправки.
pub(crate) async fn fresh(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    quoted: &PoolState,
) -> GatewayResult<()> {
    let live = load(ctx, f, pool, "latest").await?;
    if &live != quoted || !matches!(live,PoolState::Solidly{gate,..} if !gate.paused) {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "LeetSwap: изменились резервы, обеспечение или комиссия получателя",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserves_and_recipient_model_must_match_at_the_observed_block() {
        let funds = [U256::from(100), U256::from(80)];
        let zero = Address::ZERO;
        assert!(eligible(false, zero, funds, funds));
        assert!(!eligible(true, zero, funds, funds));
        assert!(!eligible(false, Address::repeat_byte(1), funds, funds));
        assert!(!eligible(
            false,
            zero,
            funds,
            [funds[0] - U256::ONE, funds[1]]
        ));
        assert!(!eligible(
            false,
            zero,
            funds,
            [funds[0], funds[1] + U256::ONE]
        ));
        assert!(!eligible(false, zero, [U256::ZERO; 2], [U256::ZERO; 2]));
    }
}
