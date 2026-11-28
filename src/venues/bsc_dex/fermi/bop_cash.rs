//! Исполнение bopAMM на Ethereum через непосредственный платёж либо USDC и Uniswap V3.
//! Оба пути используют один канонический родитель, опубликованный кадр и разрешение плательщика.
use super::{
    Builder, Ctx, Frame, Head,
    bopamm_profile::{Profile, Role},
    slot,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::primitives::{Address, B256, Bytes, U64, U256, address};
use alloy::sol_types::SolCall;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(super) const WETH: Address = address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
pub(super) const USDT: Address = address!("dac17f958d2ee523a2206206994597c13d831ec7");
pub(super) const USDC: Address = address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
pub(super) const POOL: Address = address!("3416cf6c708da44db2624d63ea0aaef7113527c6");
pub(super) const MARKET: Address = address!("b09aaa8933626d7e4c48d65dad2d77021cfbca9a");

alloy::sol! {
    struct CashOrder {bytes32 leg;bytes32 parent;uint64 height;uint64 timestamp;address input;address output;uint256 amount;uint256 minimum;}
    function execute(CashOrder o) external returns(uint256,uint256);
    function executeCash(CashOrder o) external returns(uint256,uint256);
    function quoteRoute(address input,address output,uint256 amount,bool bridge) external returns(uint256);
    function quote(address input,address output,uint256 amount) external view returns(uint256);
    event CashFilled(address indexed payer,bytes32 indexed leg,address indexed input,address output,uint256 spent,uint256 intermediate,uint256 received);
}
#[derive(Clone, Copy)]
pub(super) struct Path {
    pub profile: Profile,
    pub module: Role,
    pub bridge: bool,
}
pub(super) fn allowed(ctx: &Ctx) -> Vec<Builder> {
    super::slot_config::builders(
        ctx.settings
            .list("bopamm.slot_builders", ctx.at())
            .unwrap_or_default(),
    )
}
/// Приняты только исходные роли и байткод карточки с фактической оплатой в Ethereum.
pub(super) fn configured(ctx: &Ctx, cash: Address) -> Vec<(Builder, Path)> {
    let allowed = allowed(ctx);
    if allowed.is_empty() || cash != USDT {
        return vec![];
    }
    let Some((_, profile)) = Profile::read(&ctx.settings, ctx.place.id) else {
        return vec![];
    };
    if !approved(profile) {
        return vec![];
    }
    let legacy = profile
        .module
        .filter(|m| m.hash == super::bop_versions::DIRECT);
    let both = profile
        .cash_module
        .filter(|m| m.hash == super::bop_versions::CASH);
    let mut out = Vec::new();
    for b in allowed {
        for (module, bridge) in [(legacy, false), (both, false), (both, true)] {
            if let Some(module) = module {
                out.push((
                    b,
                    Path {
                        profile,
                        module,
                        bridge,
                    },
                ));
            }
        }
    }
    out
}
fn approved(p: Profile) -> bool {
    p.market.address == MARKET
        && (
            p.market.hash,
            p.pricing.address,
            p.pricing.hash,
            p.registry.address,
            p.registry.hash,
        ) == super::bop_versions::ROLES
}
impl Path {
    pub fn name(&self) -> &'static str {
        if self.bridge {
            "bopAMM + Uniswap V3"
        } else {
            "bopAMM"
        }
    }
    pub async fn quotes(
        &self,
        ctx: &Ctx,
        h: &Head,
        f: &Frame,
        input: Address,
        output: Address,
        sizes: &[U256],
    ) -> GatewayResult<Vec<(U256, U256)>> {
        let mut result = Vec::new();
        for batch in sizes.chunks(16) {
            let calls: Vec<_> = batch
                .iter()
                .map(|amount| {
                    let (to, data) = if self.bridge {
                        (
                            self.module.address,
                            quoteRouteCall {
                                input,
                                output,
                                amount: *amount,
                                bridge: true,
                            }
                            .abi_encode(),
                        )
                    } else {
                        (
                            self.profile.market.address,
                            quoteCall {
                                input,
                                output,
                                amount: *amount,
                            }
                            .abi_encode(),
                        )
                    };
                    json!({"from":self.module.address,"to":to,"data":Bytes::from(data)})
                })
                .collect();
            let rows = slot::run(ctx, h, f, json!(calls)).await?;
            if rows.len() != batch.len() {
                return Err(ctx.err(ErrorCode::BadResponse, "Неполная котировка bopAMM"));
            }
            for (n, q) in batch.iter().zip(rows) {
                if q.status == U64::from(1)
                    && let Ok(out) = quoteCall::abi_decode_returns(&q.return_data)
                    && !out.is_zero()
                {
                    result.push((*n, out));
                }
            }
        }
        Ok(result)
    }
    pub fn proof(
        self,
        c: &super::indicative::Candidate,
        leg: B256,
        paid: U256,
        minimum: U256,
    ) -> slot::Proof {
        let b = &c.book;
        slot::Proof {
            builder: b.config.builder.name().into(),
            parent: b.head.hash,
            target: b.frame.target,
            timestamp: b.frame.timestamp,
            executor: self.module.address,
            router: Some(self.profile.market.address),
            prop: None,
            bop: Some(Order {
                bridge: self.bridge,
                paid,
                minimum,
                runtime: self.module.hash,
            }),
            payer: c.who,
            input: c.input,
            output: c.output,
            amount: c.amount,
            limit: if c.amount.is_negative() {
                paid
            } else {
                minimum
            },
            leg,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct Order {
    pub(super) bridge: bool,
    pub(super) paid: U256,
    pub(super) minimum: U256,
    pub(super) runtime: B256,
}
impl Order {
    pub(super) fn calldata(self, p: &slot::Proof) -> Bytes {
        let o = CashOrder {
            leg: p.leg,
            parent: p.parent,
            height: p.target,
            timestamp: p.timestamp,
            input: p.input,
            output: p.output,
            amount: self.paid,
            minimum: self.minimum,
        };
        if self.bridge {
            executeCashCall { o }.abi_encode()
        } else {
            executeCall { o }.abi_encode()
        }
        .into()
    }
    pub(super) fn matches(self, p: &slot::Proof, spent: U256, received: U256) -> bool {
        let buy = p.amount.is_negative();
        let pair = if buy { (USDT, WETH) } else { (WETH, USDT) };
        (p.input, p.output) == pair
            && p.router == Some(MARKET)
            && !p.executor.is_zero()
            && !p.payer.is_zero()
            && (self.runtime == super::bop_versions::CASH
                || (!self.bridge && self.runtime == super::bop_versions::DIRECT))
            && !self.minimum.is_zero()
            && spent == self.paid
            && received >= self.minimum
            && (if buy {
                self.minimum == p.amount.unsigned_abs() && spent == p.limit
            } else {
                spent == p.amount.unsigned_abs() && self.minimum == p.limit
            })
    }
}
