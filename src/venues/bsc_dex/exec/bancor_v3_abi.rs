//! ABI of the published Bancor Network and of the protected wallet. Event fields follow the contract ABI.
#![allow(clippy::too_many_arguments)]
use alloy::sol;

sol! { interface IBancorTrade {
    struct WalletOrder {address assetIn;address assetOut;uint256 form;uint256 maximum;uint256 minimum;uint256 deadline;bool exactOutput;}
    function execute(WalletOrder order) external payable returns(uint256 spent,uint256 received);
    function network() external view returns(address);
    function info() external view returns(address);
    function vault() external view returns(address);
    function wrapped() external view returns(address);
    function nativeToken() external view returns(address);
    function tradeBySourceAmount(address source,address target,uint256 amount,uint256 minimum,uint256 deadline,address recipient) external payable returns(uint256);
    function tradeByTargetAmount(address source,address target,uint256 quantity,uint256 maximum,uint256 deadline,address recipient) external payable returns(uint256);
    function tradeBySourceAmountArb(address source,address target,uint256 amount,uint256 minimum,uint256 deadline,address recipient) external payable returns(uint256);
    function tradeByTargetAmountArb(address source,address target,uint256 quantity,uint256 maximum,uint256 deadline,address recipient) external payable returns(uint256);
    event TokensTraded(bytes32 indexed contextId,address indexed sourceToken,address indexed targetToken,uint256 sourceAmount,uint256 targetAmount,uint256 bntAmount,uint256 targetFeeAmount,uint256 bntFeeAmount,address trader);
    event Paid(address indexed payer,address indexed recipient,address indexed venue,address source,address target,uint256 form,bool exactOutput,uint256 spent,uint256 received);
} }
