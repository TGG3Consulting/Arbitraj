//! Исполнение bopAMM на Ethereum через непосредственный платёж либо USDC и Uniswap V3.
//! Оба пути используют один канонический родитель, опубликованный кадр и разрешение плательщика.
pub(crate) use super::bop_order::Order;
use super::{
    Builder, Ctx, Frame, Head,
    bop_binding::Identity,
    bopamm_profile::{Profile, Role},
    slot,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::primitives::{Address, B256, Bytes, U64, U256};
use alloy::sol_types::SolCall;
use serde_json::json;

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
    revision: u64,
    pub identity: &'static Identity,
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
    let revision = ctx.settings.generation();
    let allowed = allowed(ctx);
    if allowed.is_empty() {
        return vec![];
    }
    let Some((_, profile)) = Profile::read(&ctx.settings, ctx.place.id) else {
        return vec![];
    };
    let Some(identity) = Identity::configured(ctx, &profile) else {
        return vec![];
    };
    if cash != identity.cash.usdt.address {
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
                        module,
                        bridge,
                        revision,
                        identity,
                    },
                ));
            }
        }
    }
    out
}
impl Path {
    /// Последняя проверка перед передачей подписанного вызова. Изменение профиля/кошелька
    /// аннулирует этот билет даже при сохранении того же имени доверенного строителя.
    pub fn permitted(&self, ctx: &Ctx, builder: Builder) -> bool {
        ctx.settings.generation() == self.revision && allowed(ctx).contains(&builder)
    }
    pub fn profile(&self) -> Profile {
        self.identity.profile(self.module)
    }
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
                            self.identity.market.address,
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
            router: Some(self.identity.market.address),
            prop: None,
            bop: Some(Box::new(Order {
                source: *self.identity,
                bridge: self.bridge,
                paid,
                minimum,
                runtime: self.module.hash,
            })),
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
