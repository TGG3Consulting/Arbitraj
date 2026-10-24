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

/// Известные секреты: имя, к чему относится (площадка или «система»), описание — для интерфейса.
/// Новая площадка или сервис добавляет сюда свои секреты.
pub static KNOWN: &[(&str, &str, &str)] = &[
    (
        TELEGRAM_BOT_TOKEN,
        "telegram",
        "Токен Telegram-бота (BotFather)",
    ),
    ("bingx.api_key", "bingx", "API-ключ BingX"),
    (
        "bingx.api_secret",
        "bingx",
        "Секретный ключ BingX (Secret Key)",
    ),
    // DEX в сети BSC (адаптер bsc_dex). Узел: у провайдеров ключ стоит в адресе
    // (NodeReal: wss://bsc-mainnet.nodereal.io/ws/v1/{apiKey}, Infura: /ws/v3/<KEY> — research/flow/bsc_node.md),
    // поэтому в настройке адреса пишется {key}, а сам ключ хранится здесь.
    (
        "bsc_dex.wallet_key",
        "bsc_dex",
        "Приватный ключ кошелька бота в сети BSC (решение 45: один кошелёк)",
    ),
    (
        "bsc.node_ws_key",
        "bsc_dex",
        "Ключ провайдера узла BSC для адреса WebSocket ({key} в bsc.node_ws_url)",
    ),
    (
        "bsc.node_http_key",
        "bsc_dex",
        "Ключ провайдера узла BSC для адреса HTTP ({key} в bsc.node_http_url)",
    ),
    (
        "bsc.node_http_key_backup",
        "bsc_dex",
        "Ключ резервного провайдера узла BSC для адреса HTTP ({key} в bsc.node_http_url_backup)",
    ),
    (
        "bsc.node_ws_key_backup",
        "bsc_dex",
        "Ключ резервного провайдера узла BSC для адреса WebSocket ({key} в bsc.node_ws_url_backup)",
    ),
    // Платные каналы отправки (решение 41: канал с ключом не используется, пока ключ не введён).
    (
        "channel.blink_private.key",
        "bsc_dex",
        "Blink: ключ личного адреса bsc.blinklabs.xyz/v1/{key} (портал portal.blinklabs.xyz)",
    ),
    (
        "channel.bloxroute_cloud.key",
        "bsc_dex",
        "bloXroute Cloud API: заголовок Authorization (Account Portal)",
    ),
    (
        "channel.nodereal_api.key",
        "bsc_dex",
        "NodeReal Bundle Service API: ключ в адресе bsc-mainnet.nodereal.io/{key} (тариф Growth+)",
    ),
    (
        "channel.blockrazor_builder.key",
        "bsc_dex",
        "BlockRazor Builder: заголовок Authorization (платный доступ)",
    ),
    (
        "channel.flashblock.key",
        "bsc_dex",
        "Flashblock: заголовок Authorization (регистрация flashblock.trade)",
    ),
];
