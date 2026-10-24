//! Файл: venues/bsc_dex/pools/mod.rs
//! Что делает: пулы DEX — виды, поиск (V2 getPair, V3 PoolCreated, V4 Initialize), чтение состояния, живые
//! обновления по событиям, крючки V4 и их группы, реестр пулов в памяти.
//! Кто вызывает: mod.rs (subscribe_market, dex_families, coin_networks), позже quote/*, check/*, exec/*.
//! От чего зависит: ctx.rs, node/*, tokens.rs, math/bin_price.rs.
//! Настройки: dex.* (docs/SETTINGS.md, раздел «DEX» и «Контракты вида пула»), venue.quote_asset [bsc_dex].

pub mod abi_v2v3;
pub mod abi_v4;
pub mod attach;
pub mod bins;
pub mod created;
pub mod deploy_block;
pub mod discover;
pub mod discover_v2;
pub mod discover_v3;
pub mod discover_v4;
pub mod events;
pub mod events_bin;
pub mod events_cl;
pub mod events_v2;
pub mod events_v3;
pub mod families;
pub mod history_gate;
pub mod history_store;
pub mod history_v4;
pub mod hooks;
pub mod http_wait;
pub mod key;
pub mod link;
pub mod listen;
pub mod listen_log;
pub mod load;
pub mod load_bin;
pub mod load_cl;
pub mod load_v2v3;
pub mod market;
pub mod market_mids;
pub mod mids;
pub mod progress;
pub mod registry;
pub mod scan;
pub mod session;
pub mod ticks;
