//! Схемы LibEIP712 и LibEncoder из развёрнутого PMM. В домене 0x v2 нет chainId.
use super::abi::Order;
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use std::sync::LazyLock;

pub(super) fn domain(exchange: Address) -> B256 {
    keccak256(
        (
            keccak256("EIP712Domain(string name,string version,address verifyingContract)"),
            keccak256("0x Protocol"),
            keccak256("2"),
            exchange,
        )
            .abi_encode(),
    )
}
static ORDER_TYPE: LazyLock<B256> = LazyLock::new(|| {
    keccak256(
        "Order(address makerAddress,address takerAddress,address feeRecipientAddress,address senderAddress,uint256 makerAssetAmount,uint256 takerAssetAmount,uint256 makerFee,uint256 takerFee,uint256 expirationTimeSeconds,uint256 salt,bytes makerAssetData,bytes takerAssetData)",
    )
});
static TX_TYPE: LazyLock<B256> =
    LazyLock::new(|| keccak256("ZeroExTransaction(uint256 salt,address signerAddress,bytes data)"));

fn digest(domain: B256, body: B256) -> B256 {
    keccak256([&[0x19, 0x01][..], domain.as_slice(), body.as_slice()].concat())
}
pub(super) fn order(domain: B256, o: &Order) -> B256 {
    digest(
        domain,
        keccak256(
            (
                *ORDER_TYPE,
                o.makerAddress,
                o.takerAddress,
                o.feeRecipientAddress,
                o.senderAddress,
                o.makerAssetAmount,
                o.takerAssetAmount,
                o.makerFee,
                o.takerFee,
                o.expirationTimeSeconds,
                o.salt,
                keccak256(&o.makerAssetData),
                keccak256(&o.takerAssetData),
            )
                .abi_encode(),
        ),
    )
}
pub(super) fn transaction(domain: B256, pmm: Address, salt: U256, data: &Bytes) -> B256 {
    digest(
        domain,
        keccak256((*TX_TYPE, salt, pmm, keccak256(data)).abi_encode()),
    )
}
pub(super) fn taker(tx: B256, to: Address) -> B256 {
    keccak256([tx.as_slice(), to.as_slice()].concat())
}
