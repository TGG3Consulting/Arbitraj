//! Файл: venues/bsc_dex/wallet/mod.rs
//! Что делает: кошелёк бота в сети BSC — ключ и подпись, строгая очередь транзакций, балансы.
//! Кто вызывает: mod.rs, connection.rs, позже exec/*, send/*.
//! От чего зависит: ctx.rs, node/*.
//! Настройки: нет своих; секрет bsc_dex.wallet_key.

pub mod approve;
pub mod balances;
pub mod nonce;
pub mod signer;
pub mod transfer;
pub mod transfer_status;
