//! Файл: venues/kyberswap/signer.rs
//! Что делает: ключ кошелька бота из зашифрованных секретов → подписант alloy; адрес кошелька; подпись legacy-
//! транзакции (EIP-155, chainId BSC) и её хеш (keccak256 закодированных байт). Ключ живёт только в памяти на время
//! подписи и затирается (zeroize). Своя копия адаптера kyberswap (изоляция), на alloy напрямую.
//! Кто вызывает: nonce.rs, balances.rs, tx.rs, connection.rs.
//! От чего зависит: ctx.rs (секрет bsc_dex.wallet_key — один кошелёк, решение 45), alloy.
//! Настройки: нет. Секрет: bsc_dex.wallet_key (docs/SECURITY.md).
//!
//! Подпись — EIP-155 (https://eips.ethereum.org/EIPS/eip-155), кодирование — EIP-2718
//! (https://eips.ethereum.org/EIPS/eip-2718), хеш — keccak256 закодированных байт.

use alloy::consensus::{SignableTransaction, TxEnvelope, TxLegacy};
use alloy::eips::eip2718::Encodable2718;
use alloy::network::TxSignerSync;
use alloy::signers::local::PrivateKeySigner;
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::ctx::{Ctx, SECRET_WALLET_KEY};
use super::node::BSC_CHAIN_ID;

/// Подписанная транзакция: байты для отправки и хеш.
#[derive(Debug, Clone)]
pub struct SignedTx {
    pub raw: Bytes,
    pub hash: B256,
}

/// Подписант из секрета bsc_dex.wallet_key (64 hex-знака, можно с 0x).
/// Ошибки: SecretsLocked, NotConfigured (ключ не введён или не в формате ключа — без показа ключа).
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

/// Legacy-транзакция с chainId BSC (EIP-155). Ошибки: BadResponse — цена газа больше u128.
pub fn legacy(
    ctx: &Ctx,
    nonce: u64,
    price: U256,
    gas: u64,
    to: Address,
    value: U256,
    input: Bytes,
) -> GatewayResult<TxLegacy> {
    let gas_price = u128::try_from(price).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Цена газа {price} wei не помещается в транзакцию"),
        )
    })?;
    Ok(TxLegacy {
        chain_id: Some(BSC_CHAIN_ID),
        nonce,
        gas_price,
        gas_limit: gas,
        to: TxKind::Call(to),
        value,
        input,
    })
}

/// Подписывает транзакцию ключом кошелька. Принимает: заполненную транзакцию. Возвращает: байты EIP-2718 и хеш.
/// Ошибки: как у signer; Internal — подпись не удалась.
pub fn sign(ctx: &Ctx, mut tx: TxLegacy) -> GatewayResult<SignedTx> {
    let s = signer(ctx)?;
    let sig = s
        .sign_transaction_sync(&mut tx)
        .map_err(|e| ctx.err(ErrorCode::Internal, format!("Подпись транзакции: {e}")))?;
    let envelope = TxEnvelope::from(tx.into_signed(sig));
    let hash = *envelope.tx_hash();
    Ok(SignedTx {
        raw: Bytes::from(envelope.encoded_2718()),
        hash,
    })
}

// Явная зависимость на типаж — гарантия, что подпись возможна для legacy (одна проверка на весь модуль).
const _: fn() = || {
    fn assert_signable<T: SignableTransaction<Signature>>() {}
    assert_signable::<TxLegacy>();
};
