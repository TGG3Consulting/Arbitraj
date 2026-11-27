//! Подписанное поручение Bundles Swap. При восстановлении фактические движения сверяются с исходными пределами.
use super::super::{
    check::simulate::SimLog, ctx::Ctx, quote::v2_wallet_sim::Order, send::track::Receipt,
};
use super::{fill, signed_call};
use crate::gateway::errors::GatewayResult;
use alloy::{rpc::types::Log, sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde::{Deserialize, Serialize};

sol! { interface IExchange {
    struct Exact {uint256 amount;uint256 minimum;uint256 maxFee;address[] path;uint16[] fees;address recipient;uint256 deadline;}
    struct ModuleOrder {address pool;address input;address output;address recipient;uint256 amount;uint256 minimum;uint256 deadline;}
    function execute(ModuleOrder order,uint256 maxFeeDebit) external returns(uint256);
    function swapExactTokensForTokens(Exact order) external returns(uint256[]);
    function swapExactWETHForTokens(uint256 qty,address output,uint16 tier,address recipient,uint256 deadline,uint256 minimum) external returns(uint256);
    function swapExactTokensForWETH(address input,uint16 tier,address recipient,uint256 deadline,uint256 qty,uint256 minimum) external returns(uint256);
} }
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Terms {
    pub pool: Address,
    pub router: Address,
    pub target: Address,
    pub bun: Address,
    pub weth: Address,
    pub token0: Address,
    pub tier: u16,
    /// Максимум явной комиссии Router. Во встроенной паре он входит в цену.
    pub fee_limit: U256,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
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
            amount: o.amount,
            minimum,
            deadline,
        }
    }
}
impl Proof {
    fn embedded(&self) -> bool {
        self.input == self.terms.bun || self.output == self.terms.bun
    }
    fn separate(&self) -> bool {
        !self.embedded() && !self.terms.fee_limit.is_zero()
    }
    pub fn data(&self) -> Bytes {
        let Self {
            terms: t,
            payer: recipient,
            input,
            output,
            amount,
            minimum,
            deadline,
        } = *self;
        if t.target != t.router {
            let order = IExchange::ModuleOrder {
                pool: t.pool,
                input,
                output,
                recipient,
                amount,
                minimum,
                deadline,
            };
            IExchange::executeCall {
                order,
                maxFeeDebit: if self.separate() {
                    t.fee_limit
                } else {
                    U256::ZERO
                },
            }
            .abi_encode()
        } else if self.embedded() || self.separate() {
            IExchange::swapExactTokensForTokensCall {
                order: IExchange::Exact {
                    amount,
                    minimum,
                    maxFee: t.fee_limit,
                    path: vec![input, output],
                    fees: vec![t.tier],
                    recipient,
                    deadline,
                },
            }
            .abi_encode()
        } else if input == t.weth {
            IExchange::swapExactWETHForTokensCall {
                qty: amount,
                output,
                tier: t.tier,
                recipient,
                deadline,
                minimum,
            }
            .abi_encode()
        } else {
            IExchange::swapExactTokensForWETHCall {
                input,
                tier: t.tier,
                recipient,
                deadline,
                qty: amount,
                minimum,
            }
            .abi_encode()
        }
        .into()
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
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(&self, logs: &[SimLog]) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        let t = self.terms;
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || wallet.is_zero()
            || [t.target, t.pool, t.router].contains(&wallet)
            || input == output
            || self.minimum.is_zero()
        {
            return None;
        }
        let flow = |asset, from, to| fill::moved_logs(logs, asset, from, to);
        let debit =
            |asset| flow(asset, Some(wallet), None).checked_sub(flow(asset, None, Some(wallet)));
        let credit =
            flow(output, None, Some(wallet)).checked_sub(flow(output, Some(wallet), None))?;
        let spent = debit(input)?;
        if spent != self.amount || spent.is_zero() || credit < self.minimum {
            return None;
        }
        let fee = if self.embedded() {
            U256::ZERO
        } else {
            debit(t.bun)?
        };
        if fee > t.fee_limit || (!self.separate() && !fee.is_zero()) {
            return None;
        }
        let unique = |address: Address, sig: &str| {
            let signature = keccak256(sig.as_bytes());
            let mut v = logs
                .iter()
                .filter(|l| l.address() == address && l.topics().first() == Some(&signature));
            let event = v.next()?;
            v.next().is_none().then_some(event)
        };
        let swap = unique(
            t.pool,
            "Swap(address,uint256,uint256,uint256,uint256,address)",
        )?;
        let receiver = if !self.embedded() && !self.separate() && output == t.weth {
            t.router
        } else {
            wallet
        };
        if swap.topics().get(1..) != Some(&[t.router.into_word(), receiver.into_word()][..])
            || swap.data().data.len() != 128
        {
            return None;
        }
        let words: Vec<_> = swap
            .data()
            .data
            .chunks_exact(32)
            .map(U256::from_be_slice)
            .collect();
        let k = usize::from(input != t.token0);
        if words[k].is_zero() || words[2 + k] != U256::ZERO || words[3 - k] < credit {
            return None;
        }
        if t.target != t.router {
            let event = unique(
                t.target,
                "Filled(address,address,address,address,address,uint256,uint256,uint256)",
            )?;
            if event.topics().get(1..)
                != Some(&[wallet.into_word(), t.pool.into_word(), wallet.into_word()][..])
                || event.data().data.len() != 160
            {
                return None;
            }
            let values: Vec<_> = event
                .data()
                .data
                .chunks_exact(32)
                .map(U256::from_be_slice)
                .collect();
            if values
                != [
                    U256::from_be_slice(input.as_slice()),
                    U256::from_be_slice(output.as_slice()),
                    spent,
                    credit,
                    fee,
                ]
            {
                return None;
            }
        }
        Some((spent, credit))
    }
}
