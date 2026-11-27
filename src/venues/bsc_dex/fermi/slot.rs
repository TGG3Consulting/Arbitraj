//! Ордер одного слота Ethereum. Состояние оракула, адрес вызова и предел подписываются одним намерением.
//! Транспорт: https://docs.titanbuilder.xyz/api/eth_sendbundle и https://buildernet.org/docs/api .
use super::{Ctx, Deployment, Frame, Head, SimBlock, SimCall, deployment, quoteAmountsCall, rpc};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, I256, U64, U256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

alloy::sol! {
    struct Order {
        bytes32 leg; bytes32 parent; uint64 target; uint64 timestamp;
        address input; address output; int256 amount; uint256 limit;
    }
    function execute(Order o) external returns (uint256 spent,uint256 received);
    function completed(address payer, bytes32 leg) external view returns (bool);
    event Filled(address indexed payer, bytes32 indexed leg, address indexed input, address output, uint256 spent, uint256 received);
    event Swapped(address indexed sender, address indexed tokenIn, address indexed tokenOut, uint256 amountIn, uint256 amountOut, address recipient);
}
/// Сохранённая идентичность сети/слота и пределы обмена. Секретов и байтов подписанной транзакции здесь нет.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub builder: String,
    pub parent: B256,
    pub target: u64,
    pub timestamp: u64,
    pub executor: Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<Address>,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: I256,
    pub limit: U256,
    pub leg: B256,
}
impl Proof {
    pub fn calldata(&self) -> Bytes {
        Bytes::from(
            executeCall {
                o: Order {
                    leg: self.leg,
                    parent: self.parent,
                    target: self.target,
                    timestamp: self.timestamp,
                    input: self.input,
                    output: self.output,
                    amount: self.amount,
                    limit: self.limit,
                },
            }
            .abi_encode(),
        )
    }
    pub fn crossed(&self, spent: U256, received: U256) -> bool {
        if spent.is_zero() || received.is_zero() {
            return false;
        }
        let specified = self.amount.unsigned_abs();
        if self.amount.is_negative() {
            received == specified && spent <= self.limit
        } else {
            spent == specified && received >= self.limit
        }
    }
}

pub(super) async fn run(
    ctx: &Ctx,
    head: &Head,
    frame: &Frame,
    calls: Value,
) -> GatewayResult<Vec<SimCall>> {
    let tag = json!({"blockHash":head.hash,"requireCanonical":true});
    let body = json!({"blockStateCalls":[{"blockOverrides":{"time":U64::from(frame.timestamp)},"stateOverrides":frame.overrides(),"calls":calls}]});
    let mut blocks: Vec<SimBlock> = rpc::call(ctx, "eth_simulateV1", json!([body, tag])).await?;
    let Some(b) = blocks.pop() else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Fermi: отсутствует результат симуляции",
        ));
    };
    if !blocks.is_empty()
        || b.parent_hash != head.hash
        || b.number.to::<u64>() != frame.target
        || b.timestamp.to::<u64>() != frame.timestamp
    {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Fermi: симуляция выполнена на другом слоте",
        ));
    }
    Ok(b.calls)
}
/// Сама цена рассчитывается с исполняющего адреса. Движок Fermi видит адрес вызывающего контракта.
pub(super) async fn amounts(
    ctx: &Ctx,
    head: &Head,
    frame: &Frame,
    caller: Address,
    input: Address,
    output: Address,
    amount: I256,
) -> GatewayResult<(U256, U256)> {
    let q = quoteAmountsCall {
        tokenIn: input,
        tokenOut: output,
        amountSpecified: amount,
    };
    let rows = run(
        ctx,
        head,
        frame,
        json!([{"from":caller,"to":frame.deployment.router,"data":Bytes::from(q.abi_encode())}]),
    )
    .await?;
    let [r] = rows.as_slice() else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Fermi: отсутствует результат котировки",
        ));
    };
    let q = quoteAmountsCall::abi_decode_returns(&r.return_data)
        .ok()
        .filter(|q| {
            r.status == U64::from(1u8)
                && !q.amountIn.is_zero()
                && !q.amountOut.is_zero()
                && if amount.is_negative() {
                    q.amountOut == amount.unsigned_abs()
                } else {
                    q.amountIn == amount.unsigned_abs()
                }
        })
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Fermi: в кадре нет цены на весь объём"))?;
    Ok((q.amountIn, q.amountOut))
}

pub(super) async fn verify_executor(
    ctx: &Ctx,
    head: &Head,
    executor: Address,
    deployment: Deployment,
    runtime: B256,
) -> GatewayResult<()> {
    let tag = json!({"blockHash":head.hash,"requireCanonical":true});
    let (code, router, engine): (Bytes, Address, Address) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([executor, tag])),
        deployment::executor_router(ctx, head.hash, executor),
        deployment::executor_engine(ctx, head.hash, executor)
    )?;
    if code.is_empty()
        || alloy_primitives::keccak256(code) != runtime
        || (router, engine) != (deployment.router, deployment.engine)
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Fermi: код или привязки исполнителя расходятся с профилем",
        ));
    }
    Ok(())
}

pub(super) async fn simulate(
    ctx: &Ctx,
    head: &Head,
    frame: &Frame,
    order: &Proof,
    fees: Option<(u128, u128)>,
) -> GatewayResult<(U256, U256, u64)> {
    let mut call = json!({"from":order.payer,"to":order.executor,"data":order.calldata()});
    if let Some((cap, tip)) = fees {
        call["maxFeePerGas"] = json!(U256::from(cap));
        call["maxPriorityFeePerGas"] = json!(U256::from(tip));
    }
    let rows = run(ctx, head, frame, json!([call])).await?;
    let [r] = rows.as_slice() else {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Fermi: нет единственного результата исполнения",
        ));
    };
    let fill = executeCall::abi_decode_returns(&r.return_data)
        .ok()
        .filter(|v| {
            r.status == U64::from(1) && order.crossed(v.spent, v.received) && !r.gas_used.is_zero()
        })
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "Fermi: симуляция кошелька не подтвердила обмен и ценовой предел",
            )
        })?;
    Ok((fill.spent, fill.received, r.gas_used.to::<u64>()))
}
