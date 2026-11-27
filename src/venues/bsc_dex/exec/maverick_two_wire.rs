//! Поручение официальному Maverick V2 Router и суммы неиндексированного PoolSwap.
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};

pub(super) const EVENT: (&str, usize) = (
    "PoolSwap(address,address,(uint256,bool,bool,int32),uint256,uint256)",
    256,
);

sol! {
    interface Router {
        function checkDeadline(uint256 deadline) external;
        function exactInputSingle(address recipient,address pool,bool tokenAIn,uint256 amountIn,uint256 amountOutMinimum) external;
        function multicall(bytes[] calls) external;
    }
}

pub(super) fn order(
    pool: Address,
    a_in: bool,
    amount: U256,
    minimum: U256,
    wallet: Address,
    deadline: U256,
) -> Bytes {
    let deadline = Router::checkDeadlineCall { deadline };
    let trade = Router::exactInputSingleCall {
        recipient: wallet,
        pool,
        tokenAIn: a_in,
        amountIn: amount,
        amountOutMinimum: minimum,
    };
    Router::multicallCall {
        calls: vec![deadline.abi_encode().into(), trade.abi_encode().into()],
    }
    .abi_encode()
    .into()
}

pub(super) fn parties(data: &[u8], router: Address, wallet: Address) -> bool {
    data.get(..32) == Some(router.into_word().as_slice())
        && data.get(32..64) == Some(wallet.into_word().as_slice())
}

pub(super) fn amounts(data: &[u8], side: usize, spent: U256, received: U256, budget: U256) -> bool {
    if data.len() != EVENT.1 || side > 1 {
        return false;
    }
    let word = |i: usize| U256::from_be_slice(&data[32 * i..32 * (i + 1)]);
    let limit = if side == 0 {
        U256::from(i32::MAX as u32)
    } else {
        U256::ZERO.wrapping_sub(U256::from(1u64 << 31))
    };
    spent == budget
        && word(2) == budget
        && word(3) == U256::from((side == 0) as u8)
        && word(4).is_zero()
        && word(5) == limit
        && word(6) > U256::ZERO
        && word(6) <= spent
        && word(7) == received
}

#[cfg(test)]
#[path = "maverick_two_wire_tests.rs"]
mod tests;
