//! Файл: venues/bsc_dex/wallet/mod.rs
//! Что делает: кошелёк бота в сети BSC — ключ и подпись, строгая очередь транзакций, балансы, ожидаемые поступления,
//! изменения баланса событиями (решение 76).
//! Кто вызывает: mod.rs, connection.rs, exec/*, send/*, quote/*, resubscribe.rs.
//! От чего зависит: ctx.rs, node/*.
//! Настройки: нет своих; секрет bsc_dex.wallet_key.

pub mod allow_cache;
pub mod approve;
pub mod balances;
pub mod deposit;
pub mod events;
pub mod events_log;
pub mod events_native;
pub mod events_tx;
pub mod inflows;
pub mod native_status;
pub mod nonce;
pub mod queue_store;
pub mod signer;
pub mod transfer;
pub mod transfer_cost;
pub mod transfer_status;
