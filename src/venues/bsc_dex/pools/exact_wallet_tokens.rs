//! Ethereum ERC-20 implementations where a successful transfer or transferFrom credits its full input amount.
use super::solidly_classic_factory::plain;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, address, b256};
use serde_json::Value;

fn accepted() -> [(Address, B256); 4] {
    [
        (
            address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"),
            b256!("d0a06b12ac47863b5c7be4185c2deaad1c61557033f56c7d4ea74429cbb25e23"),
        ), // WETH
        (
            address!("2260fac5e5542a773aa44fbcfedf7c193bc2c599"),
            b256!("131ff5c755b710d543ea70fede2eb38e5d15b1456df0ae932ba12e2786f7e5df"),
        ), // WBTC
        (
            address!("6b175474e89094c44da98b954eedeac495271d0f"),
            b256!("4e36f96ee1667a663dfaac57c4d185a0e369a3a217e0079d49620f34f85d1ac7"),
        ), // DAI
        (
            address!("5f98805a4e8be255a32880fdec7f6728c6568ba0"),
            b256!("a607f4c8379ffb91323184b39d374287ea702b699648610ba87b29fd4f9b00a1"),
        ), // LUSD
    ]
}

fn runtime(token: Address) -> Option<B256> {
    accepted()
        .into_iter()
        .find_map(|(a, h)| (a == token).then_some(h))
}
pub(crate) fn eligible(input: Address, output: Address) -> bool {
    input != output && runtime(input).is_some() && runtime(output).is_some()
}
pub(crate) async fn check(
    ctx: &Ctx,
    input: Address,
    output: Address,
    at: &Value,
) -> GatewayResult<()> {
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Не подтверждена полная выплата ERC-20 на кошелёк",
        )
    };
    if ctx.chain_id() != Some(1) || input == output {
        return Err(bad());
    }
    let source = runtime(input).ok_or_else(bad)?;
    let target = runtime(output).ok_or_else(bad)?;
    tokio::try_join!(
        plain(ctx, input, source, at),
        plain(ctx, output, target, at)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audited_hashes_and_both_token_directions_are_bound_to_ethereum() {
        let json: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/ethereum_exact_transfers.json"
        ))
        .unwrap();
        for v in json["tokens"].as_object().unwrap().values() {
            let a: Address = v["address"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                runtime(a),
                Some(v["keccak256"].as_str().unwrap().parse().unwrap())
            );
            assert!(
                v["sources"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|x| x.as_str().is_some_and(|s| s.len() == 64))
            );
        }
        let weth = accepted()[0].0;
        let dai = accepted()[2].0;
        assert!(eligible(weth, dai) && eligible(dai, weth));
        for token in [
            address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"),
            address!("dac17f958d2ee523a2206206994597c13d831ec7"),
        ] {
            assert!(!eligible(token, weth) && !eligible(weth, token));
        }
    }
}
