//! Защищённый вызов конкретного shared HanjiExecutor. Симуляция проходит от кошелька без подмены состояния.
use super::{book::Head, live::canonical};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U64, U256, address, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::json;
sol! {
    struct Order { address market; address input; address output; uint256 budget; uint256 maxUnspent;
        uint256 minReceived; uint72 limitPrice; uint128 maxCommission; uint256 deadline; bytes32 operationId; }
    function executeExact(Order order,uint256 maxReceived) external returns(uint256 spent,uint256 received,uint128 feeUnits);
    function wrapped() external view returns(address);
    function balanceOf(address holder) external view returns(uint256);
    event Filled(address indexed payer,bytes32 indexed operationId,address indexed market,
        address input,address output,uint256 spent,uint256 received,uint128 feeUnits);
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Proof {
    pub executor: Address,
    pub payer: Address,
    pub market: Address,
    pub input: Address,
    pub output: Address,
    pub budget: U256,
    pub max_unspent: U256,
    pub min_received: U256,
    pub max_received: U256,
    pub price: u128,
    pub max_commission: u128,
    pub deadline: u64,
    pub leg: B256,
}
impl Proof {
    pub(super) fn data(&self) -> Option<Bytes> {
        Some(Bytes::from(
            executeExactCall {
                order: Order {
                    market: self.market,
                    input: self.input,
                    output: self.output,
                    budget: self.budget,
                    maxUnspent: self.max_unspent,
                    minReceived: self.min_received,
                    limitPrice: self.price.try_into().ok()?,
                    maxCommission: self.max_commission,
                    deadline: U256::from(self.deadline),
                    operationId: self.leg,
                },
                maxReceived: self.max_received,
            }
            .abi_encode(),
        ))
    }
    pub(super) fn amounts_ok(&self, spent: U256, received: U256, fee: u128) -> bool {
        !spent.is_zero()
            && !received.is_zero()
            && self
                .budget
                .checked_sub(spent)
                .is_some_and(|v| v <= self.max_unspent)
            && received >= self.min_received
            && received <= self.max_received
            && fee <= self.max_commission
    }
}
pub(super) async fn verify(ctx: &Ctx, h: &Head, executor: Address) -> GatewayResult<()> {
    let (code, wrapped): (Bytes, Address) = tokio::try_join!(
        rpc::call(
            ctx,
            "eth_getCode",
            json!([executor,{"blockHash":h.hash,"requireCanonical":true}])
        ),
        canonical(ctx, executor, wrappedCall {}, h.hash)
    )?;
    let hash = keccak256(code);
    if Some(hash) != include_str!("runtime.txt").trim().parse::<B256>().ok()
        || wrapped != address!("4200000000000000000000000000000000000006")
    {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Hanji: код shared executor или адрес WETH отличается от проверенного на Base",
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimBlock {
    parent_hash: B256,
    number: U64,
    timestamp: U64,
    calls: Vec<Row>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    status: U64,
    return_data: Bytes,
    gas_used: U64,
}
pub(super) async fn simulate(
    ctx: &Ctx,
    h: &Head,
    q: &Proof,
) -> GatewayResult<(U256, U256, u128, u64)> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Реальный вызов кошелька Hanji не подтверждает границы и итоговые балансы",
        )
    };
    let balance = |token| json!({"from":q.payer,"to":token,"input":Bytes::from(balanceOfCall{holder:q.payer}.abi_encode())});
    let bi = balance(q.input);
    let bo = balance(q.output);
    let swap = json!({"from":q.payer,"to":q.executor,"data":q.data().ok_or_else(bad)?});
    let rows:Vec<SimBlock>=rpc::call(ctx,"eth_simulateV1",json!([{"blockStateCalls":[{"calls":[bi,bo,swap,bi,bo]}]},{"blockHash":h.hash,"requireCanonical":true}])).await?;
    let [block] = rows.as_slice() else {
        return Err(bad());
    };
    let [i, o, r, i2, o2] = block.calls.as_slice() else {
        return Err(bad());
    };
    if block.parent_hash != h.hash
        || block.number != h.number + U64::from(1)
        || block.timestamp.to::<u64>() > q.deadline
        || block.calls.iter().any(|r| r.status != U64::from(1))
    {
        return Err(bad());
    }
    let b = |r: &Row| {
        (r.return_data.len() == 32)
            .then(|| U256::from_be_slice(&r.return_data))
            .ok_or_else(bad)
    };
    let ret = executeExactCall::abi_decode_returns(&r.return_data).map_err(|_| bad())?;
    if !q.amounts_ok(ret.spent, ret.received, ret.feeUnits)
        || b(i)?.checked_sub(b(i2)?) != Some(ret.spent)
        || b(o2)?.checked_sub(b(o)?) != Some(ret.received)
    {
        return Err(bad());
    }
    Ok((ret.spent, ret.received, ret.feeUnits, r.gas_used.to()))
}
