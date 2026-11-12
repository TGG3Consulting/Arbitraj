//! Файл: venues/bsc_dex/check/mod.rs
//! Что делает: проверки монет и пулов (решения 30, 32): налоги покупки/продажи/перевода пробным обменом
//! (eth_simulateV1), газ обмена, влияние крючков V4 группы B/C (квотер против математики), циклы перепроверки.
//! Торговли нет, пока проверка не завершена.
//! Кто вызывает: pools/market.rs (запуск циклов), mod.rs (recheck_coins), quote/* (итоги).
//! От чего зависит: ctx.rs, node/*, pools/*, quote/*, exec/calls.rs.
//! Настройки: tax.*, hooks.*, check.probe_usdt, dex.quoter_check_s, dex.quoter_max_diff_pct, dex.deadline_s,
//! money.max_per_trade_usd (сумма сверки с квотером, решение 261).

pub mod cycle;
pub mod deal_taxes;
pub mod emitter;
pub mod fresh;
pub mod hooks_probe;
pub mod native_probe;
pub mod pool_taxes;
pub mod probe;
pub mod probe_buy;
pub mod probe_calls;
pub mod probe_exec;
pub mod quoter_amount;
pub mod quoter_block;
pub mod quoter_compare;
pub mod quoter_cycle;
pub mod quoter_pause;
pub mod quoter_side;
pub mod quoter_sides;
pub mod quoter_tally;
pub mod quoter_verify;
pub mod recheck;
pub mod service_tax;
pub mod service_tax_calls;
pub mod sim_base;
pub mod simulate;
pub mod state;
pub mod tax_cycle;
pub mod tax_deep;
pub mod tax_pass;
pub mod tax_wait;
pub mod tax_wallet;
pub mod taxes;
pub mod taxes_util;
