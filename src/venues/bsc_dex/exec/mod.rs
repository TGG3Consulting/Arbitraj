//! Файл: venues/bsc_dex/exec/mod.rs
//! Что делает: исполнение обмена на DEX — прямой обмен в одном пуле через Universal Router своей DEX
//! (V2, V3, V4) с проверкой реально полученного баланса в той же транзакции, разрешения Permit2, сделка целиком.
//! Сухая сделка DEX (решение 121, C4) — dry_swap.rs: сборка как для сделки и симуляция без отправки.
//! Кто вызывает: check/probe.rs, mod.rs (trade), venue.rs (dex_dry_swap).
//! От чего зависит: node/abi_dex.rs, pools/*.
//! Настройки: dex.router, dex.deadline_s.

pub mod bundle;
pub mod bundle_notify;
pub mod bundle_send;
pub mod bundle_sim;
pub mod bundle_trade;
pub mod chain;
pub mod chain_tx;
pub mod cycle_bundle;
pub mod dry_build;
pub mod dry_fund;
pub mod dry_judge;
pub mod dry_paths;
pub mod dry_sim;
pub mod dry_size;
pub mod dry_swap;
pub mod fill;
pub mod fork_swap;
pub mod live;
pub mod native;
pub mod order;
pub mod plan;
pub mod precheck;
pub mod precheck_sim;
pub mod result;
pub mod swap_v2v3;
pub mod swap_v4;
pub mod trade;
pub mod trade_smart;
pub mod txs;
pub mod universal;
pub mod ur;
pub mod verify_gate;
