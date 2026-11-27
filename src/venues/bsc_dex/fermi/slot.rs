//! Ордер одного слота Ethereum. Состояние оракула, адрес вызова и предел подписываются одним намерением.
//! Транспорт: https://docs.titanbuilder.xyz/api/eth_sendbundle и https://buildernet.org/docs/api .
//! Котировщик и восстановление используют общий ABI и пределы подписанного кошельком поручения.
#[path = "slot_simulation.rs"]
mod simulation;
pub(super) use simulation::{amounts, run, simulate, verify_executor};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, I256, U256};
use serde::{Deserialize, Serialize};

alloy::sol! {
    struct Order {
        bytes32 leg; bytes32 parent; uint64 target; uint64 timestamp;
        address input; address output; int256 amount; uint256 limit;
    }
    function execute(Order o) external returns (uint256 spent,uint256 received);
    struct PropOrder { bytes32 leg; bytes32 parent; uint64 height; uint64 timestamp; address tokenIn; address tokenOut; uint256 amount; uint256 minimum; }
    function executeAtSlot(PropOrder order) external returns(uint256 spent,uint256 received);
    function slotVersion() external view returns(uint256);
    event SlotFilled(bytes32 indexed leg,address indexed payer,address indexed venue,uint256 spent,uint256 received);
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prop: Option<Box<super::super::exec::prop_wallet::Proof>>,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: I256,
    pub limit: U256,
    pub leg: B256,
}
impl Proof {
    pub fn calldata(&self) -> Bytes {
        if let Some(quote) = &self.prop {
            return executeAtSlotCall {
                order: PropOrder {
                    leg: self.leg,
                    parent: self.parent,
                    height: self.target,
                    timestamp: self.timestamp,
                    tokenIn: quote.input,
                    tokenOut: quote.output,
                    amount: quote.amount,
                    minimum: quote.minimum,
                },
            }
            .abi_encode()
            .into();
        }
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
        if let Some(q) = &self.prop {
            if q.payer != self.payer
                || q.terms.target != self.executor
                || q.terms.fallback_allowed
                || self.router != Some(q.terms.router)
                || q.terms.module_hash.is_none()
                || (q.input, q.output) != (self.input, self.output)
                || q.deadline != U256::from(self.timestamp)
                || spent != q.amount
                || received < q.minimum
            {
                return false;
            }
            return if self.amount.is_negative() {
                q.minimum == specified && spent <= self.limit
            } else {
                spent == specified && q.minimum == self.limit
            };
        }
        if self.amount.is_negative() {
            received == specified && spent <= self.limit
        } else {
            spent == specified && received >= self.limit
        }
    }
}
