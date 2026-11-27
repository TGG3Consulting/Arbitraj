//! Физические состояния Bundles Swap для одного канонического блока.
use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Market {
    pub pool: Address,
    pub tokens: [Address; 2],
    pub tier: u16,
    pub reserves: [U256; 2],
    /// Сырые ERC20 balanceOf пары в порядке token0, token1, BUN.
    pub balances: [U256; 3],
    pub fee_reserve: U256,
    pub supply: U256,
    /// Индекс токена пары, оцениваемого оракулом в BUN, и цена в UQ112x112.
    pub oracle: Option<(usize, U256)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub market: Market,
    pub fee_pair: Market,
    pub bun: Address,
    pub weth: Address,
    pub router_balance: U256,
    pub deposits: U256,
}
