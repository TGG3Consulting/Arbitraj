//! Файл: venues/bsc_dex/wallet/signer.rs
//! Что делает: ключ кошелька бота из зашифрованных секретов → подписант alloy; адрес кошелька;
//! подпись транзакции и её хеш (считается локально до отправки — research/flow/bsc_send.md, раздел 5).
//! Ключ живёт только в памяти на время подписи и затирается (zeroize); в журнал, ошибки и логи не попадает.
//! Кто вызывает: wallet/balances.rs, wallet/nonce.rs, wallet/approve.rs, wallet/transfer.rs, exec/*, connection.rs.
//! От чего зависит: ctx.rs (секреты), alloy (signer-local, consensus, network, eips).
//! Настройки: нет. Секрет: bsc_dex.wallet_key (docs/SECURITY.md, решение 45 — один кошелёк).
//!
//! Подпись — EIP-155 (защита от повтора в другой сети: узел BSC принимает по RPC только такие —
//! https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/internal/ethapi/api.go#L2315-L2324 ),
//! кодирование — EIP-2718 (https://eips.ethereum.org/EIPS/eip-2718), хеш — keccak256 закодированных байт.

use alloy::consensus::{SignableTransaction, TxEnvelope};
use alloy::eips::eip2718::Encodable2718;
use alloy::network::TxSignerSync;
use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Address, B256, Bytes, Signature};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::{Ctx, SECRET_WALLET_KEY};

/// Подписанная транзакция: байты для отправки и хеш.
#[derive(Debug, Clone)]
pub struct SignedTx {
    pub raw: Bytes,
    pub hash: B256,
}

/// Подписант из секрета bsc_dex.wallet_key (64 hex-знака, можно с 0x).
/// Ошибки: SecretsLocked, NotConfigured (ключ не введён или не в формате ключа — без показа самого ключа).
pub fn signer(ctx: &Ctx) -> GatewayResult<PrivateKeySigner> {
    let key = ctx.vault.get(SECRET_WALLET_KEY).map_err(|e| ctx.tag(e))?;
    key.trim().parse::<PrivateKeySigner>().map_err(|_| {
        ctx.err(
            ErrorCode::NotConfigured,
            format!("Секрет {SECRET_WALLET_KEY}: это не приватный ключ (нужно 64 шестнадцатеричных знака)"),
        )
    })
}

/// Адрес кошелька бота. Ошибки: как у signer.
pub fn address(ctx: &Ctx) -> GatewayResult<Address> {
    Ok(signer(ctx)?.address())
}

/// Подписывает транзакцию любого вида (legacy, EIP-1559) ключом кошелька.
/// Принимает: транзакцию с заполненными nonce, газом, chain_id. Возвращает: байты и хеш.
/// Ошибки: как у signer; Internal — подпись не удалась (текст alloy, без ключа).
pub fn sign<T>(ctx: &Ctx, tx: T) -> GatewayResult<SignedTx>
where
    T: SignableTransaction<Signature>,
    TxEnvelope: From<alloy::consensus::Signed<T>>,
{
    sign_with(&signer(ctx)?, tx)
        .map_err(|e| ctx.err(ErrorCode::Internal, format!("Подпись транзакции: {e}")))
}

/// Подпись данным подписантом (без обращения к секретам).
/// Принимает: подписанта, транзакцию. Возвращает: байты EIP-2718 и хеш. Ошибка — текст alloy.
pub fn sign_with<T>(s: &PrivateKeySigner, mut tx: T) -> Result<SignedTx, String>
where
    T: SignableTransaction<Signature>,
    TxEnvelope: From<alloy::consensus::Signed<T>>,
{
    let sig = s
        .sign_transaction_sync(&mut tx)
        .map_err(|e| e.to_string())?;
    let envelope = TxEnvelope::from(tx.into_signed(sig));
    let hash = *envelope.tx_hash();
    Ok(SignedTx {
        raw: Bytes::from(envelope.encoded_2718()),
        hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::consensus::TxLegacy;
    use alloy_primitives::{TxKind, U256, address, hex};

    /// Официальный пример EIP-155 (https://eips.ethereum.org/EIPS/eip-155, раздел «Example»):
    /// ключ 0x4646…46, nonce 9, gasPrice 20 gwei, газ 21000, получатель 0x3535…35, 1 ether, chainId 1.
    #[test]
    fn eip155_example() {
        let key: PrivateKeySigner =
            "0x4646464646464646464646464646464646464646464646464646464646464646"
                .parse()
                .unwrap();
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 9,
            gas_price: 20_000_000_000,
            gas_limit: 21_000,
            to: TxKind::Call(address!("0x3535353535353535353535353535353535353535")),
            value: U256::from(1_000_000_000_000_000_000u128),
            input: Bytes::new(),
        };
        let signed = sign_with(&key, tx).unwrap();
        assert_eq!(
            hex::encode_prefixed(&signed.raw),
            "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
        );
    }
}
