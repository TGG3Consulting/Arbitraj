//! ABI одной принятой формы tradeRFQT и атомарного расчёта. Восьмипараметровое событие относится к интерфейсу контракта.
#![allow(clippy::too_many_arguments)]
use alloy::sol;

sol! {
    struct OrderV6 {
        address pool;
        address maker;
        address recipient;
        address input;
        address output;
        uint256 sell;
        uint256 buy;
        uint256 minimum;
        uint256 expiry;
        uint256 nonce;
        uint256[10] confidence;
        bytes16 quoteId;
        bool multiHop;
        uint256 mode;
        bytes signature;
        address feeRecipient;
        uint256 feeRate;
        bytes feeSignature;
    }
    interface IGuard {
        function execute(address input,address output,uint256 amount,uint256 minimum,uint256 deadline,bytes offer) external returns(uint256);
        event Settled(bytes16 indexed quoteId,address indexed payer,address indexed maker,bytes32 offerDigest,address input,address output,uint256 spent,uint256 received);
    }
    event RFQTrade(address recipient,address soldToken,address boughtToken,uint256 sold,uint256 bought,bytes16 quoteId,address maker);
}
