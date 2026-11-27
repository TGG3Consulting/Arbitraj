//! Родные поручения MCV2_Bond. Покупка фиксирует выпуск и потолок платежа, продажа — сжигаемую сумму и минимум выплаты.
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

sol! {
    interface IBondFlow {
        function execute(address token,address reserve,bool mint,uint256 qty,uint256 maxDebit,uint256 minCredit,uint256 deadline) external returns(uint256 debit,uint256 credit);
        function bond() external view returns(address);
        function cloneHash() external view returns(bytes32);
        event Settled(address indexed payer,address indexed token,address indexed reserve,bool mint,uint256 qty,uint256 debit,uint256 credit);
        function mint(address token,uint256 tokensToMint,uint256 maxReserveAmount,address receiver) external returns(uint256);
        function burn(address token,uint256 tokensToBurn,uint256 minRefund,address receiver) external returns(uint256);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Protection {
    pub module: Address,
    pub bond: Address,
    pub deadline: U256,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Call {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protection: Option<Protection>,
    pub token: Address,
    pub reserve: Address,
    pub mint: bool,
    pub qty: U256,
    /// Максимальный дебет резерва на mint, минимальный чистый кредит на burn.
    pub limit: U256,
}
impl Call {
    pub fn data(self, wallet: Address) -> Bytes {
        if let Some(p) = self.protection {
            return IBondFlow::executeCall {
                token: self.token,
                reserve: self.reserve,
                mint: self.mint,
                qty: self.qty,
                maxDebit: if self.mint { self.limit } else { self.qty },
                minCredit: if self.mint { self.qty } else { self.limit },
                deadline: p.deadline,
            }
            .abi_encode()
            .into();
        }
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
