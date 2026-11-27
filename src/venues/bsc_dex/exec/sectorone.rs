//! Единственный ERC-20 шаг через LBRouter с контролем чистого прихода на кошелёк.
use super::{
    chain::Step,
    plan::{DirectCall, Plan},
    white_direct,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, pools::sectorone, quote::white_state},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
alloy::sol! {
    interface ILegacyWallet {
        function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn,uint256 amountOutMin,uint256[] binSteps,address[] tokenPath,address to,uint256 deadline) external returns(uint256);
    }
    interface IWallet {
        struct Path {uint256[] pairBinSteps;uint8[] versions;address[] tokenPath;}
        function swapExactTokensForTokensSupportingFeeOnTransferTokens(uint256 amountIn,uint256 amountOutMin,Path path,address to,uint256 deadline) external returns(uint256);
    }
}
#[derive(Clone, Copy, Debug)]
pub(in crate::venues::bsc_dex) struct Path {
    pub legacy: bool,
    pub step: u32,
}
impl Path {
    pub(in crate::venues::bsc_dex) fn data(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        to: Address,
        deadline: U256,
    ) -> Bytes {
        let steps = vec![U256::from(self.step)];
        let tokens = vec![input, output];
        if self.legacy {
            ILegacyWallet::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                amountIn: amount,
                amountOutMin: minimum,
                binSteps: steps,
                tokenPath: tokens,
                to,
                deadline,
            }
            .abi_encode()
            .into()
        } else {
            // У этого LBRouter V2_1=2 направляет шаг в собственную фабрику с хуками.
            let path = IWallet::Path {
                pairBinSteps: steps,
                versions: vec![2],
                tokenPath: tokens,
            };
            IWallet::swapExactTokensForTokensSupportingFeeOnTransferTokensCall {
                amountIn: amount,
                amountOutMin: minimum,
                path,
                to,
                deadline,
            }
            .abi_encode()
            .into()
        }
    }
}

pub(in crate::venues::bsc_dex) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [s] = steps else {
        return Err(ctx.err(ErrorCode::NoRoute, "LBRouter требует один пул"));
    };
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, "latest").await?;
    let (legacy, step) = sectorone::bin(&s.entry)
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "LBRouter: не определён шаг бина"))?;
    let router = ctx.dex_addr("dex.router", s.entry.family.id)?;
    sectorone::check(ctx, &s.entry, &h.selector(), true).await?;
    if s.entry.family.variant==crate::venues::bsc_dex::pools::family_variant::Variant::E3Lb{
        crate::venues::bsc_dex::pools::e3::input(ctx,s.entry.pool.reference.address(),s.order.token_in,&h.selector()).await?;
    }
    let mut plan = white_direct::build(ctx, s, wallet, router, now, all, generation).await?;
    plan.set_direct_market(DirectCall::SectorLb {
        path: Path { legacy, step },
        input: s.order.token_in,
        amount: s.order.amount_in,
    });
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use serde_json::Value;

    #[test]
    fn native_calldata_matches_archived_wallets_and_strict_minima() {
        let captures: [&[u8]; 4] = [
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/sectorone-20-base.json.gz"
            ),
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/sectorone-22-base.json.gz"
            ),
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/sectorone-20-ethereum.json.gz"
            ),
            include_bytes!(
                "../../../../tests/data/first_run_dex_2026-11-26/sectorone-22-ethereum.json.gz"
            ),
        ];
        let mut counts = Vec::new();
        for raw in captures {
            let d: Value = serde_json::from_reader(GzDecoder::new(raw)).unwrap();
            let wallet = &d["settlement"];
            let owner = wallet["owner"].as_str().unwrap().parse().unwrap();
            let path = Path {
                legacy: d["directory"]["version"] == "2.0",
                step: wallet["step"].as_u64().unwrap() as u32,
            };
            let mut count = 0;
            for c in wallet["cases"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["mode"] == "router")
            {
                for leg in ["buy", "sell"].iter().filter_map(|side| c.get(*side)) {
                    let calls = leg["calls"].as_array().unwrap();
                    let r = leg["results"].as_array().unwrap();
                    let n = calls.len();
                    let x = calls[n - 2]["to"].as_str().unwrap().parse().unwrap();
                    let y = calls[n - 1]["to"].as_str().unwrap().parse().unwrap();
                    let paid: Bytes = calls[n - 3]["data"].as_str().unwrap().parse().unwrap();
                    let idx = 4 + 32 * if path.legacy { 5 } else { 4 };
                    let deadline = U256::from_be_slice(&paid[idx..idx + 32]);
                    let amount = leg["input"].as_str().unwrap().parse().unwrap();
                    let minimum: U256 = leg["net"].as_str().unwrap().parse().unwrap();
                    assert_eq!(path.data(x, y, amount, minimum, owner, deadline), paid);
                    let too_high: Bytes = calls[n - 5]["data"].as_str().unwrap().parse().unwrap();
                    assert_eq!(
                        path.data(x, y, amount, minimum + U256::ONE, owner, deadline),
                        too_high
                    );
                    assert_eq!(r[n - 5]["status"], "0x0");
                    assert_eq!(r[n - 4]["status"], "0x0");
                    assert_eq!(r[n - 3]["status"], "0x1");
                    count += 1;
                }
            }
            counts.push(count);
        }
        assert_eq!(counts, [4, 4, 4, 3]);
    }
}
