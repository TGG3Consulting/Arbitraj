//! Файл: secrets/mod.rs
//! Что делает: секреты (ключи бирж, ключ кошелька, токен Telegram) — в зашифрованном файле
//! secrets.enc, расшифровка паролем при запуске, значения только в памяти.
//! Кто вызывает: api/secrets.rs, notify, позже адаптеры.
//! От чего зависит: crypto.rs (Argon2id + XChaCha20-Poly1305), file.rs, vault.rs.
//! Настройки: security.kdf_memory_kib, security.kdf_iterations, security.kdf_parallelism,
//! security.password_min_len. Подробности — docs/SECURITY.md, решение — docs/DECISIONS.md №27.

pub mod crypto;
pub mod file;
pub mod vault;

pub use vault::Vault;

/// Имя секрета токена Telegram-бота.
pub const TELEGRAM_BOT_TOKEN: &str = "telegram.bot_token";
