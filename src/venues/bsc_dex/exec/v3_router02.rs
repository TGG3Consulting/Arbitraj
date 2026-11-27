//! SwapRouter02 verifies the deadline in its multicall envelope.
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256, aliases::{I24,U24}};
use serde::{Deserialize,Serialize};

sol! {
    interface FeeRouter {
        struct Order {address tokenIn;address tokenOut;uint24 fee;address recipient;uint256 amountIn;uint256 amountOutMinimum;uint160 sqrtPriceLimitX96;}
        function exactInputSingle(Order params) external payable returns (uint256);
        function multicall(uint256 deadline, bytes[] data) external payable returns (bytes[]);
    }
    interface SpacingRouter {
        struct Order {address tokenIn;address tokenOut;int24 tickSpacing;address recipient;uint256 amountIn;uint256 amountOutMinimum;uint160 sqrtPriceLimitX96;}
        function exactInputSingle(Order params) external payable returns (uint256);
    }
}

#[derive(Debug,Clone,Copy,Deserialize,Serialize)]
pub enum Key {
    Fee,
    Spacing(I24),
}

impl Key {
    pub(super) fn order(self, fee: U24, (input,output):(Address,Address), amount:U256,
                        minimum:U256, wallet:Address, deadline:U256) -> Bytes {
        let inner = match self {
            Self::Fee => FeeRouter::exactInputSingleCall {params:FeeRouter::Order {
                tokenIn:input, tokenOut:output, fee, recipient:wallet, amountIn:amount,
                amountOutMinimum:minimum, sqrtPriceLimitX96:Default::default(),
            }}.abi_encode(),
            Self::Spacing(tick_spacing) => SpacingRouter::exactInputSingleCall {params:SpacingRouter::Order {
                tokenIn:input, tokenOut:output, tickSpacing:tick_spacing, recipient:wallet, amountIn:amount,
                amountOutMinimum:minimum, sqrtPriceLimitX96:Default::default(),
            }}.abi_encode(),
        };
        FeeRouter::multicallCall {deadline,data:vec![inner.into()]}.abi_encode().into()
    }
}
