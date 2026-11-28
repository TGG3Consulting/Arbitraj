//! Форматы подписей токенов 0x v2 из наблюдавшихся исполнений PMM.
use alloy_primitives::{Address, B256, Bytes, Signature, U256, eip191_hash_message};
/// Перестановка VRS (0x v2) в представление подписи Alloy.
pub(super) fn vrs(s: &[u8]) -> Option<Signature> {
    (s.len() >= 65 && matches!(s[0], 27 | 28)).then(|| {
        Signature::new(
            U256::from_be_slice(&s[1..33]),
            U256::from_be_slice(&s[33..65]),
            s[0] == 28,
        )
    })
}
pub(super) fn maker_signature(s: &Bytes, order_hash: B256, maker: Address) -> bool {
    match (s.last(), s.len()) {
        // Wallet: право внутреннего подписанта определяет сам контракт maker во время исходного fill.
        (Some(4), 88) => true,
        (Some(tag @ (2 | 3)), 66) => {
            let digest = if *tag == 3 {
                eip191_hash_message(order_hash)
            } else {
                order_hash
            };
            vrs(s).and_then(|sig| sig.recover_address_from_prehash(&digest).ok()) == Some(maker)
        }
        _ => false,
    }
}
