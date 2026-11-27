//! Файл: venues/bsc_dex/exec/mod.rs
//! Что делает: исполнение обмена на DEX — прямой обмен в одном пуле через Universal Router своей DEX
//! (V2, V3, V4) с проверкой реально полученного баланса в той же транзакции, разрешения Permit2, сделка целиком.
//! Сухая сделка DEX (решение 121, C4) — dry_swap.rs: сборка как для сделки и симуляция без отправки.
//! Свой контракт-исполнитель ArbExecutor (решение 122, часть X) — executor/: одна транзакция по пулам разных DEX.
//! Кто вызывает: check/probe.rs, mod.rs (trade), venue.rs (dex_dry_swap).
//! От чего зависит: node/abi_dex.rs, pools/*.
//! Настройки: dex.router, dex.deadline_s.

pub mod approve_seen;
pub mod balance_fact;
pub mod balance_read;
pub mod bundle;
pub mod bundle_native;
pub mod bundle_notify;
pub mod bundle_rebuild;
pub mod bundle_ref;
pub mod bundle_send;
pub mod bundle_sim;
pub mod bundle_trade;
pub mod chain;
pub mod chain_tx;
pub mod cycle_bundle;
pub mod cycle_bundle_trade;
pub mod cycle_dry;
pub mod cycle_exec;
pub mod cycle_gas;
pub mod cycle_gate;
pub mod cycle_ref;
pub mod cycle_segment;
pub mod cycle_steps;
pub mod cycle_trade;
pub(super) mod delta_direct;
pub(super) mod sectorone;
pub(super) mod umbrae;
#[cfg(test)]
mod umbrae_tests;
pub(super) mod vault_direct;
pub(super) mod ekubo_v2_direct;
pub(super) mod ekubo_v2_fact;
mod signed_call;
pub(in crate::venues::bsc_dex) mod registered_v3_wallet;
pub(in crate::venues::bsc_dex) mod solidly_router_wallet;
pub(super) mod carbon_binding;
pub(super) mod carbon_wire;
pub(super) mod carbon_direct;
pub(super) mod carbon_fact;
pub(super) mod dagrow_wallet;
pub(super) mod bundles_wallet;
#[cfg(test)]
mod bundles_wallet_tests;
pub(super) mod bundles_plan;
#[cfg(test)]
mod dagrow_wallet_tests;
pub(super) mod temple_wallet;
pub(super) mod pendle_wallet;
pub(super) mod o1_wallet;
pub(super) mod flux_eth;
pub(super) mod flux_eth_plan;
#[cfg(test)]
mod flux_eth_tests;
mod o1_plan;
#[cfg(test)]
mod o1_wallet_tests;
pub(super) mod antfarm_wallet;
pub(super) mod antfarm_output_wallet;
#[cfg(test)]
mod temple_wallet_tests;
#[cfg(test)]
mod pendle_wallet_tests;
pub(super) mod mint_club_wire;
pub(super) mod mint_club_fact;
pub(super) mod mint_club_direct;
pub(super) mod mint_club_binding;
pub mod dry_build;
pub mod dry_build_once;
pub mod dry_checks;
pub mod dry_fund;
pub mod dry_fund_gather;
pub mod dry_fund_holder;
pub mod dry_judge;
pub mod dry_paths;
pub mod dry_sim;
pub mod dry_size;
pub mod dry_surplus;
pub mod dry_surplus_cut;
pub mod dry_surplus_tail;
pub mod dry_swap;
pub mod executor;
pub mod exit_gap;
pub mod exit_quote;
pub mod fill;
pub mod fork_swap;
pub(super) mod frax_direct;
pub mod gas_fund;
pub mod hook_drive;
pub mod hook_trade;
pub mod leg_min;
pub mod live;
pub mod live_hops;
pub mod native;
pub mod native_exec;
pub(super) mod oasis_direct;
pub mod order;
pub mod permit2_err;
pub mod plan;
pub mod pool_cond;
pub mod pool_cond_check;
pub mod pool_cond_decode;
pub mod pool_cond_read;
pub mod pool_cond_row;
pub mod pool_cond_stat;
pub mod pool_cond_stat_store;
pub mod pool_cond_topics;
pub mod preapprove;
pub mod precheck;
pub mod precheck_quoter;
pub mod precheck_sim;
pub mod presell;
pub mod presell_credit;
pub mod presell_slot;
pub mod presell_slot_find;
pub mod quoter_favour;
pub mod quoter_refetch;
pub mod quoter_refetch_check;
pub mod quoter_refetch_ekubo;
pub mod quoter_refetch_note;
pub mod quoter_refetch_proof;
pub(super) mod market_plan;
pub(super) mod market_approval;
pub(super) mod market_receipt;
pub mod result;
pub mod settlement_swap;
pub mod single_rebuild;
pub mod smart_legs;
pub mod solidly_direct;
pub mod solidly_gate;
pub mod swap_ctx;
pub mod swap_rebuild;
pub mod swap_send;
pub mod swap_v2v3;
pub mod swap_v4;
pub mod tip_cycle;
pub mod tip_judge;
pub mod tip_pick;
pub mod tip_state;
pub mod tip_state_store;
pub mod trade;
pub mod trade_direct;
pub mod trade_direct_prep;
pub mod trade_hooks;
pub mod trade_probe;
pub mod trade_probe_plan;
pub mod trade_smart;
pub mod tx_fee;
pub mod txs;
pub mod universal;
pub mod ur;
pub mod ur_remeasure;
pub mod ur_short;
pub mod verify_gate;
pub(super) mod vista_direct;
pub(crate) mod vista_fact;
mod vista_gate;
#[cfg(test)]
mod vista_receipt_tests;
pub(crate) mod vista_wire;
pub(super) mod white_direct;
