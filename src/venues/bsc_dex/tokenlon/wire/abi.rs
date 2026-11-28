//! ABI проверенных UserProxy, PMM и 0x Exchange v2 Ethereum.
use alloy::sol;
#[allow(clippy::too_many_arguments)]
mod exact {
    use super::sol;
    sol! {
        struct Order {
            address makerAddress; address takerAddress; address feeRecipientAddress; address senderAddress;
            uint256 makerAssetAmount; uint256 takerAssetAmount; uint256 makerFee; uint256 takerFee;
            uint256 expirationTimeSeconds; uint256 salt; bytes makerAssetData; bytes takerAssetData;
        }
        function fillOrKillOrder(Order order, uint256 takerAssetFillAmount, bytes signature);
        function fill(uint256 userSalt, bytes data, bytes userSignature);
        function toPMM(bytes payload);
        event FillOrder(string source, bytes32 indexed transactionHash, bytes32 indexed orderHash,
            address indexed userAddr, address takerAssetAddr, uint256 takerAssetAmount, address makerAddr,
            address makerAssetAddr,uint256 makerAssetAmount,address receiverAddr,uint256 settleAmount,uint16 feeFactor);
        event Fill(address indexed makerAddress,address indexed feeRecipientAddress,address takerAddress,
            address senderAddress,uint256 makerAssetFilledAmount,uint256 takerAssetFilledAmount,
            uint256 makerFeePaid,uint256 takerFeePaid,bytes32 indexed orderHash,bytes makerAssetData,bytes takerAssetData);
        event Deposit(address indexed dst,uint256 wad);
        event Withdrawal(address indexed src,uint256 wad);
    }
}
pub(in crate::venues::bsc_dex::tokenlon) use exact::*;
