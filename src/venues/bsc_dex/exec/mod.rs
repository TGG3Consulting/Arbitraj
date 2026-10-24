//! Файл: venues/bsc_dex/exec/mod.rs
//! Что делает: исполнение обмена на DEX — прямой обмен в одном пуле через Universal Router своей DEX
//! (V2, V3, V4) с проверкой реально полученного баланса в той же транзакции, разрешения Permit2, сделка целиком.
//! Кто вызывает: check/probe.rs, mod.rs (trade).
//! От чего зависит: node/abi_dex.rs, pools/*.
//! Настройки: dex.router, dex.deadline_s.

pub mod chain;
pub mod fill;
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
