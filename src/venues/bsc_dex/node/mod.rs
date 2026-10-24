//! Файл: venues/bsc_dex/node/mod.rs
//! Что делает: связь с узлом BSC — JSON-RPC по HTTP и WebSocket, подписки, головы сети, вызовы контрактов.
//! Кто вызывает: все части адаптера DEX.
//! От чего зависит: ctx.rs, errors.rs.
//! Настройки: bsc.node_ws_url, bsc.node_http_url, bsc.node_http_rps (node/pace.rs), http.timeout_ms, venue.ws_* [bsc_dex], feed.stale_after_ms [bsc_dex].

pub mod abi;
pub mod abi_dex;
pub mod call;
pub mod failover;
pub mod heads;
pub mod jsonrpc;
pub mod multicall;
pub mod pace;
pub mod rpc;
pub mod subs;
pub mod ws;
pub mod ws_task;

/// Chain ID сети BSC mainnet — значение протокола: «HTTP mainnet (ChainID 56)» —
/// https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/ (research/flow/bsc_node.md, раздел 1).
/// Им подписываются транзакции (EIP-155) и сверяется узел в «Проверить подключение».
pub const BSC_CHAIN_ID: u64 = 56;
