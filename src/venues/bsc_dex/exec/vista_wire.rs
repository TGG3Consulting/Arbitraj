//! Денежный ABI EtherVista. Native BUY у official router платит ETH целиком,
//! native SELL изолирует остаток роутера через принятый модуль, ERC20 использует swapFrom.
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

sol! {interface IVistaFlow {
    function swapExactETHForTokensSupportingFeeOnTransferTokens(uint256 minimum,address[] calldata path,address recipient,uint256 deadline) external payable;
    function swapNative(address tokenIn,address tokenOut,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external payable returns(uint256);
    function swapFrom(address tokenIn,address tokenOut,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external returns(uint256);
    event NativeSettled(address indexed payer,address indexed recipient,address indexed tokenIn,address tokenOut,uint256 spent,uint256 received,uint256 fee);
}}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Form {
    OfficialEthIn,
    ModuleEthOut,
    ModuleToken,
}
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct Call {
    pub form: Form,
    pub wrapped: Address,
    pub fee: U256,
}
impl Call {
    pub fn value(self, amount: U256) -> U256 {
        match self.form {
            Form::OfficialEthIn => amount,
            Form::ModuleEthOut => self.fee,
            Form::ModuleToken => U256::ZERO,
        }
    }
    pub fn data(
        self,
        input: Address,
        output: Address,
        amount: U256,
        minimum: U256,
        recipient: Address,
        deadline: U256,
    ) -> Bytes {
        match self.form {
            Form::OfficialEthIn => {
                IVistaFlow::swapExactETHForTokensSupportingFeeOnTransferTokensCall {
                    minimum,
                    path: vec![input, output],
                    recipient,
                    deadline,
                }
                .abi_encode()
            }
            Form::ModuleEthOut => IVistaFlow::swapNativeCall {
                tokenIn: input,
                tokenOut: output,
                amount,
                minimum,
                recipient,
                deadline,
            }
            .abi_encode(),
            Form::ModuleToken => IVistaFlow::swapFromCall {
                tokenIn: input,
                tokenOut: output,
                amount,
                minimum,
                recipient,
                deadline,
            }
            .abi_encode(),
        }
        .into()
    }
}
