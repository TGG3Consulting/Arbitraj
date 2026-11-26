//! Родные поручения MCV2_Bond. Покупка фиксирует выпуск и потолок платежа, продажа — сжигаемую сумму и минимум выплаты.
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

sol! {
    interface IBondFlow {
        function mint(address token,uint256 tokensToMint,uint256 maxReserveAmount,address receiver) external returns(uint256);
        function burn(address token,uint256 tokensToBurn,uint256 minRefund,address receiver) external returns(uint256);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Call {
    pub token: Address,
    pub reserve: Address,
    pub mint: bool,
    pub qty: U256,
    /// Максимальный дебет резерва на mint, минимальный чистый кредит на burn.
    pub limit: U256,
}
impl Call {
    pub fn data(self, wallet: Address) -> Bytes {
        if self.mint {
            IBondFlow::mintCall {
                token: self.token,
                tokensToMint: self.qty,
                maxReserveAmount: self.limit,
                receiver: wallet,
            }
            .abi_encode()
            .into()
        } else {
            IBondFlow::burnCall {
                token: self.token,
                tokensToBurn: self.qty,
                minRefund: self.limit,
                receiver: wallet,
            }
            .abi_encode()
            .into()
        }
    }
    pub fn pair(self) -> (Address, Address) {
        if self.mint {
            (self.reserve, self.token)
        } else {
            (self.token, self.reserve)
        }
    }
    pub fn tighten(self, spent: U256, received: U256) -> Option<Self> {
        let limit = if self.mint {
            spent.checked_sub(U256::ONE)?
        } else {
            received.checked_add(U256::ONE)?
        };
        Some(Self { limit, ..self })
    }
}
