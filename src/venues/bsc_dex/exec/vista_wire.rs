//! Денежный ABI EtherVista. Native BUY у official router платит ETH целиком,
//! native SELL изолирует остаток роутера через принятый модуль, ERC20 использует swapFrom.
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};

sol! {interface IVistaFlow {
    function swapExactETHForTokensSupportingFeeOnTransferTokens(uint256 minimum,address[] calldata path,address recipient,uint256 deadline) external payable;
    function swapExactTokensForETHSupportingFeeOnTransferTokens(uint256 amount,uint256 minimum,address[] calldata path,address recipient,uint256 deadline) external payable;
    function swapExactETHForTokens(uint256 minimum,address to,uint256 deadline) external payable returns(uint256);
    function swapExactTokensForETH(uint256 amount,uint256 minimum,address to,uint256 deadline) external returns(uint256);
    function swapNative(address tokenIn,address tokenOut,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external payable returns(uint256);
    function swapFrom(address tokenIn,address tokenOut,uint256 amount,uint256 minimum,address recipient,uint256 deadline) external returns(uint256);
    function _buy(uint256 minimum,uint256 deadline) external payable returns(bool);
    function _sell(uint256 amount,uint256 deadline,uint256 grossMinimum) external returns(bool);
    event Bought(address indexed buyer,address indexed market,uint256 qty,uint256 eth,uint256 statistic);
    event Sold(address indexed market,address indexed seller,uint256 qty,uint256 eth,uint256 statistic);
    event Filled(address indexed sender,address indexed recipient,address indexed input,address output,uint256 spent,uint256 received);
    event NativeSettled(address indexed payer,address indexed recipient,address indexed tokenIn,address tokenOut,uint256 spent,uint256 received,uint256 fee);
}}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Form {
    OfficialEthIn,
    OfficialEthOut,
    ModuleEthOut,
    ModuleToken,
    RabbitEthIn,
    RabbitEthOut,
    SurgeEthIn,
    SurgeEthOut,
    SurgeToken,
    SurgeModuleEthIn,
    SurgeModuleEthOut,
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
            Form::OfficialEthIn | Form::RabbitEthIn | Form::SurgeEthIn | Form::SurgeModuleEthIn => amount,
            Form::OfficialEthOut | Form::ModuleEthOut => self.fee,
            Form::ModuleToken | Form::RabbitEthOut | Form::SurgeEthOut | Form::SurgeToken | Form::SurgeModuleEthOut => U256::ZERO,
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
            Form::OfficialEthOut => {
                IVistaFlow::swapExactTokensForETHSupportingFeeOnTransferTokensCall {
                    amount,
                    minimum: minimum.saturating_add(self.fee),
                    path: vec![input, output],
                    recipient,
                    deadline,
                }
                .abi_encode()
            }
            Form::RabbitEthIn => IVistaFlow::swapExactETHForTokensCall {
                minimum,
                to: recipient,
                deadline,
            }
            .abi_encode(),
            Form::RabbitEthOut => IVistaFlow::swapExactTokensForETHCall {
                amount,
                minimum,
                to: recipient,
                deadline,
            }
            .abi_encode(),
            Form::ModuleEthOut | Form::SurgeModuleEthIn | Form::SurgeModuleEthOut => IVistaFlow::swapNativeCall {
                tokenIn: input,
                tokenOut: output,
                amount,
                minimum,
                recipient,
                deadline,
            }
            .abi_encode(),
            Form::SurgeEthIn => IVistaFlow::_buyCall{minimum,deadline}.abi_encode(),
            Form::SurgeEthOut => IVistaFlow::_sellCall{
                amount,deadline,grossMinimum:super::super::math::surge::gross_for_net(minimum,90).unwrap_or(U256::MAX)
            }.abi_encode(),
            Form::ModuleToken | Form::SurgeToken => IVistaFlow::swapFromCall {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn official_native_sell_minimum_includes_the_fee_paid_by_the_recipient() {
        let snapshots = [
            include_str!("../../../../tests/data/first_run_dex_2026-11-26/ethervista_base.json"),
            include_str!(
                "../../../../tests/data/first_run_dex_2026-11-26/ethervista_ethereum.json"
            ),
        ];
        for snapshot in snapshots {
            let raw: Value = serde_json::from_str(snapshot).unwrap();
            let obs = &raw["observations"][0];
            let output: Address = raw["weth"].as_str().unwrap().parse().unwrap();
            let input: Address = obs["token"].as_str().unwrap().parse().unwrap();
            let fee = U256::from(obs["fees"]["sell"]["native_wei"].as_u64().unwrap());
            let form = Call {
                form: Form::OfficialEthOut,
                wrapped: output,
                fee,
            };
            let mut signs = Vec::new();
            for row in obs["swaps"].as_array().unwrap() {
                let gross: U256 = row["measured_gross_wei"].as_str().unwrap().parse().unwrap();
                let net: i128 = row["sell_same_wallet"]["wallet_native"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                let cost = fee.to::<i128>();
                assert_eq!(gross.to::<i128>() - cost, net);
                assert_eq!(row["sell_same_wallet"]["router_weth_before"], "0");
                assert_eq!(row["sell_min_plus_one"]["status"], "0x0");
                assert_eq!(row["sell_min_plus_one"]["wallet_native"], "0");
                assert_eq!(row["sell_fee_minus_one"]["status"], "0x0");
                assert!(row["sell_same_wallet"]["gas"].as_u64().unwrap() > 0);
                signs.push(net > 0);
                if net <= 0 {
                    continue;
                }
                let amount: U256 = row["buy"]["wallet_tokens"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                let recorded =
                    alloy_primitives::keccak256(b"Arbitraj Base 8453 isolated owner 2026-11-26");
                let recipient = Address::from_slice(&recorded.as_slice()[12..]);
                let encoded = form.data(
                    input,
                    output,
                    amount,
                    U256::from(net as u128),
                    recipient,
                    U256::ONE,
                );
                let parsed =
                    IVistaFlow::swapExactTokensForETHSupportingFeeOnTransferTokensCall::abi_decode(
                        &encoded,
                    )
                    .unwrap();
                assert_eq!(parsed.minimum, gross);
                assert_eq!(parsed.amount, amount);
                assert_eq!(parsed.path, vec![input, output]);
                assert_eq!(parsed.recipient, recipient);
                assert_eq!(form.value(amount), fee);
            }
            assert!(signs.contains(&false) && signs.contains(&true));
        }
    }
}
