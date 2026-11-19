//! Файл: venues/bsc_dex/node/mod.rs
//! Что делает: связь с узлом BSC — JSON-RPC по HTTP и WebSocket, подписки, головы сети, вызовы контрактов.
//! Кто вызывает: все части адаптера DEX.
//! От чего зависит: ctx.rs, errors.rs.
//! Настройки: bsc.node_ws_url, bsc.node_http_url, bsc.node_http_rps (node/pace.rs), http.timeout_ms, venue.ws_* [bsc_dex], feed.stale_after_ms [bsc_dex].

pub mod abi;
pub mod abi_dex;
pub mod block_ms;
pub mod call;
pub mod chain_id;
pub mod chain_seen;
pub mod events_lag;
pub mod failover;
pub mod head_age;
pub mod head_stall;
pub mod head_stall_send;
pub mod head_stall_text;
pub mod heads;
pub mod jsonrpc;
pub mod log_filter;
pub mod multicall;
pub mod pace;
pub mod prefetch;
pub mod queue;
pub mod receipts;
pub mod receipts_budget;
pub mod receipts_chain;
pub mod receipts_fetch;
pub mod receipts_handle;
pub mod receipts_hold;
pub mod receipts_io;
pub mod receipts_lag;
pub mod receipts_nodes;
pub mod receipts_notice;
pub mod receipts_pipe;
pub mod receipts_step;
pub mod receipts_task;
pub mod req_trail;
pub mod rpc;
pub mod rpc_capped;
pub mod rpc_race;
pub mod source;
pub mod subs;
pub mod ws;
pub mod ws_task;

// Решение 416: chainId сети — настройка dex.evm_chain_id площадки DEX (chain_id.rs), а не константа 56 в коде.
