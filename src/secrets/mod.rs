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
    ("mexc.api_key", "mexc", "API-ключ MEXC (Access Key)"),
    (
        "mexc.api_secret",
        "mexc",
        "Секретный ключ MEXC (Secret Key)",
    ),
    ("gate.api_key", "gate", "API-ключ Gate (Key, API v4)"),
    (
        "gate.api_secret",
        "gate",
        "Секретный ключ Gate (Secret, API v4)",
    ),
    ("bybit.api_key", "bybit", "API-ключ Bybit (API Key, HMAC)"),
    (
        "bybit.api_secret",
        "bybit",
        "Секретный ключ Bybit (API Secret)",
    ),
    ("kucoin.api_key", "kucoin", "API-ключ KuCoin (API Key)"),
    (
        "kucoin.api_secret",
        "kucoin",
        "Секретный ключ KuCoin (API Secret)",
    ),
    (
        "kucoin.api_passphrase",
        "kucoin",
        "Passphrase ключа KuCoin (задаётся при создании ключа; в запросе — зашифрованной HMAC-SHA256)",
    ),
    (
        "bitget.api_key",
        "bitget",
        "API-ключ Bitget (API Key, Classic v2)",
    ),
    (
        "bitget.api_secret",
        "bitget",
        "Секретный ключ Bitget (Secret Key, подпись HMAC SHA256)",
    ),
    (
        "bitget.api_passphrase",
        "bitget",
        "Passphrase ключа Bitget (задаётся при создании ключа; заголовок ACCESS-PASSPHRASE)",
    ),
    ("okx.api_key", "okx", "API-ключ OKX (API key, API v5)"),
    ("okx.api_secret", "okx", "Секретный ключ OKX (Secret key)"),
    (
        "okx.api_passphrase",
        "okx",
        "Passphrase ключа OKX (задаётся при создании ключа; в запросе — заголовок OK-ACCESS-PASSPHRASE)",
    ),
    (
        "binance.api_key",
        "binance",
        "API-ключ Binance (API Key, тип HMAC)",
    ),
    (
        "binance.api_secret",
        "binance",
        "Секретный ключ Binance (Secret Key; подпись HMAC SHA256)",
    ),
    (
        "xt.api_key",
        "xt",
        "API-ключ XT.com (AppKey; заголовок validate-appkey)",
    ),
    (
        "xt.api_secret",
        "xt",
        "Секретный ключ XT.com (SecretKey; подпись HmacSHA256)",
    ),
    (
        "poloniex.api_key",
        "poloniex",
        "API-ключ Poloniex (Spot API Key; заголовок key)",
    ),
    (
        "poloniex.api_secret",
        "poloniex",
        "Секретный ключ Poloniex (Secret Key; подпись HmacSHA256)",
    ),
    (
        "phemex.api_key",
        "phemex",
        "API-ключ Phemex (API Key — поле id на странице ключей; заголовок x-phemex-access-token)",
    ),
    (
        "phemex.api_secret",
        "phemex",
        "Секретный ключ Phemex (API Secret; подпись HMAC SHA256)",
    ),
    (
        "kraken.api_key",
        "kraken",
        "API-ключ Kraken (API Key, спот)",
    ),
    (
        "kraken.api_secret",
        "kraken",
        "Приватный ключ Kraken (Private Key, Base64; подпись HMAC-SHA512)",
    ),
    (
        "coinex.api_key",
        "coinex",
        "access_id ключа CoinEx (API v2; заголовок X-COINEX-KEY)",
    ),
    (
        "coinex.api_secret",
        "coinex",
        "secret_key ключа CoinEx (подпись HMAC-SHA256; сам секрет не передаётся)",
    ),
    // DEX в сети BSC (адаптер bsc_dex). Узел: у провайдеров ключ стоит в адресе
    // (NodeReal: wss://bsc-mainnet.nodereal.io/ws/v1/{apiKey}, Infura: /ws/v3/<KEY> — research/flow/bsc_node.md),
    // поэтому в настройке адреса пишется {key}, а сам ключ хранится здесь.
    (
        "bsc_dex.wallet_key",
        "bsc_dex",
        "Приватный ключ прежнего кошелька бота в сети BSC — с решения 132 кошелёк «BingX» (ключи новых кошельков — wallet.<id>.key, экран «Маппинг кошельков»)",
    ),
    (
        "index.api_key",
        "bsc_dex",
        "Ключ CoinGecko для справочника пулов (решение 58): заголовок — настройка index.key_header; без неё ключ не нужен",
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
    // DEX через агрегатор KyberSwap (адаптер kyberswap, решение 91). Кошелёк — кошелёк счёта сделки (решение 132),
    // отдельного ключа кошелька у площадки нет. Узел BSC для исполнения — свой адрес.
    (
        "kyber.node_http_key",
        "kyberswap",
        "Ключ провайдера узла BSC для адреса HTTP агрегатора KyberSwap ({key} в kyber.node_http_url)",
    ),
];
