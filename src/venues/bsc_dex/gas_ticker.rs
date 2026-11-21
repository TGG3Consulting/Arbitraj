//! Файл: venues/bsc_dex/gas_ticker.rs
//! Что делает: решение 416 (ревью шага 1, решение техлида) — тикер монеты газа сети площадки DEX из настройки
//! dex.gas_ticker (BNB у BSC, ETH у Arbitrum) вместо константы «BNB» (бывший wallet/balances.rs BNB). Этим тикером
//! адаптер пишет газ, чаевые и газ перевода в затратах (Cost.asset) и в комиссии сделки (Fee), строку газа в балансах;
//! ядро сравнивает их с тем же dex.gas_ticker (core/gas_ticker.rs). dex.native_ticker (нативная монета на бирже,
//! решения 62/270/276) — отдельно, не трогается.
//! Решение 398: после отправки транзакции настройки не читаются — затраты газа после отправки (exec/result.rs, перевод)
//! берут тикер, прочитанный перед ней (node/chain_seen.rs ensure → ticker), — sent.
//! Кто вызывает: wallet/{balances,transfer,transfer_cost,transfer_status}.rs, exec/result.rs, resolve_withdraw.rs,
//! quote/{mod,pick,steps,native_gas}.rs, best_of/extra.rs, pools/mid_dex.rs, node/chain_seen.rs.
//! От чего зависит: ctx.rs (настройки, Ctx.chains — память тикера).
//! Настройки: dex.gas_ticker [bsc_dex].

use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;

/// Что делает: тикер газа из настроек площадки (котировка, балансы, затраты перевода — до отправки; без памяти — на
/// горячем пути котировки нет замка). Принимает: окружение. Возвращает: тикер. Ошибки: NotConfigured — не задана, пуста.
pub fn read(ctx: &Ctx) -> GatewayResult<String> {
    let t = ctx.text("dex.gas_ticker")?;
    if t.trim().is_empty() {
        return Err(ctx.not_set(
            "dex.gas_ticker",
            "dex.gas_ticker пуста — задайте тикер газа сети",
        ));
    }
    Ok(t)
}

/// Что делает: тикер газа из настроек и в память окружения — перед отправкой транзакции (node/chain_seen.rs ensure),
/// для затрат после неё (sent). Принимает: окружение. Возвращает: тикер. Ошибки: как у read.
pub fn ticker(ctx: &Ctx) -> GatewayResult<String> {
    let t = read(ctx)?;
    ctx.chains.note_gas(&t);
    Ok(t)
}

/// Что делает: тикер газа для затрат после отправки — прочитанный перед ней (без чтения настроек, решение 398); в
/// памяти нет (позднее выяснение после перезапуска, без отправки в этом процессе) — из настроек. Принимает: окружение.
/// Возвращает: тикер. Ошибки: как у read (только если в памяти нет).
pub fn sent(ctx: &Ctx) -> GatewayResult<String> {
    match ctx.chains.gas() {
        Some(t) => Ok(t),
        None => read(ctx),
    }
}
