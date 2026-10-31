//! Файл: venues/bsc_dex/quote/cycle_scan.rs
//! Что делает: поиск замкнутых циклов DEX ↔ DEX по всем базовым монетам кошелька и сбор их сигналов (решение 85, фаза
//! сигнала — без исполнения). За переключателем направления dex_dex.enabled (решение 85: каждое направление включается
//! отдельно): не задано/выключено — поиск не работает (пусто, как везде в проекте). Строит тот же граф пулов всех DEX
//! (pools/graph.rs, решение 80), для каждой базовой монеты (dex_dex.base_tokens) перечисляет циклы base → … → base
//! (quote/route_paths::enumerate_cycles) в пределах dex_dex.max_cycle_len / dex_dex.max_cycles, считает каждый цикл
//! (quote/cycle.rs) на пробном входе dex_dex.probe_input_base и собирает сигналы. Оба обхода цикла — отдельными
//! сигналами (разные первые рёбра), считаются независимо (решение 85).
//! Через налоговые токены цикл не строим (как мост через промежуточную — quote/via.rs): каждый НЕбазовый токен цикла
//! должен быть проверенной монетой без налогов (покупки/продажи/перевода); иначе цикл пропускается — «налог токена на
//! каждом свопе» (решение 85) тогда структурно равен нулю, не гадаем о неописанной механике мид-свопного налога.
//! Газ свопов и чаевые (BNB) переводятся в базовую монету курсом из графа (решение 62: WBNB ≈ BNB; для базовой USDT —
//! лучший пул WBNB–USDT, quote/rate.rs). Нет перевода — чистой прибыли нет, видна валовая (quote/cycle.rs).
//! Первичный фильтр BingX ↔ DEX (монета на бирже / выводится по BSC) к DEX ↔ DEX НЕ применяется (решение 85): нужна
//! только ликвидность пулов и совпадение адресов токенов в пулах (это обеспечивает граф и реестр).
//! Исполнение цикла (бандл) — следующая фаза; этот код авто-сделки НЕ запускает.
//! Кто вызывает: слой стратегии/сигнала DEX ↔ DEX (следующий стык, решение 85); тесты — напрямую quote/cycle.rs.
//! От чего зависит: quote/cycle.rs, quote/route_paths.rs, quote/rate.rs, pools/graph.rs, pools/registry.rs, tokens.rs,
//! send/mod.rs (чаевые канала), num.rs.
//! Настройки: dex_dex.enabled, dex_dex.base_tokens, dex_dex.max_cycle_len, dex_dex.max_cycles, dex_dex.probe_input_base,
//! dex_dex.min_profit_base [bsc_dex]; через граф — dex.route_max_pools_per_edge и глубина (решение 73).

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::CheckState;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::graph;
use super::super::pools::registry::Registry;
use super::Quoting;
use super::cycle::{self, CycleSignal};
use super::{rate, route_paths};

/// Включено ли направление DEX ↔ DEX (решение 85). Не задано — выключено (как dry.enabled). Ошибок нет.
pub fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("dex_dex.enabled").unwrap_or(false)
}

/// Пределы перебора циклов из настроек. Ошибки: NotConfigured (не заданы или вне допустимого).
pub fn limits(ctx: &Ctx) -> GatewayResult<(usize, usize)> {
    let max_len = usize::try_from(ctx.int("dex_dex.max_cycle_len")?).unwrap_or(0);
    let max_cycles = usize::try_from(ctx.int("dex_dex.max_cycles")?).unwrap_or(0);
    if max_len < 2 {
        return Err(ctx.not_set(
            "dex_dex.max_cycle_len",
            "dex_dex.max_cycle_len должна быть не меньше 2 (цикл — минимум 2 хопа)",
        ));
    }
    if max_cycles == 0 {
        return Err(ctx.not_set(
            "dex_dex.max_cycles",
            "dex_dex.max_cycles должна быть больше нуля",
        ));
    }
    Ok((max_len, max_cycles))
}

/// Поиск циклов по всем базовым монетам и сбор сигналов. Принимает: окружение котировки и цену газа (wei, как при
/// отправке — см. quote/mod.rs). Возвращает: сигналы циклов, прибыльные (dex_dex.min_profit_base) впереди, затем по
/// убыванию чистой прибыли. Выключено (dex_dex.enabled) — пусто. Ошибки: NotConfigured (пределы, базовые монеты,
/// пробный вход), узел (decimals), BadResponse (числа).
pub async fn scan(q: &Quoting, gas: U256) -> GatewayResult<Vec<CycleSignal>> {
    let ctx = &q.ctx;
    if !enabled(ctx) {
        return Ok(Vec::new());
    }
    let (max_len, max_cycles) = limits(ctx)?;
    let input = ctx.dec("dex_dex.probe_input_base")?;
    if input <= Decimal::ZERO {
        return Err(ctx.not_set(
            "dex_dex.probe_input_base",
            "dex_dex.probe_input_base должна быть больше нуля",
        ));
    }
    let min_profit = ctx.dec("dex_dex.min_profit_base").unwrap_or(Decimal::ZERO);
    let usdt = q.tokens.usdt(ctx)?;
    let usdt_dec = q.tokens.decimals(ctx, usdt).await?;
    let bases = ctx
        .settings
        .addresses("dex_dex.base_tokens", ctx.at())
        .map_err(|e| ctx.tag(e))?;
    if bases.is_empty() {
        return Err(ctx.not_set(
            "dex_dex.base_tokens",
            "dex_dex.enabled = да, а список dex_dex.base_tokens пуст",
        ));
    }
    let g = graph::build(ctx, &q.reg, &q.tokens, usdt_dec)?;
    let wbnb = ctx.addr("dex.native_wrapped").ok();
    let tickers = coin_tickers(&q.reg);
    let tip = tip_bnb(ctx);
    let mut out = Vec::new();
    for base in bases {
        let cycles = route_paths::enumerate_cycles(&g, base, max_len, max_cycles);
        let base_ticker = tickers
            .get(&base)
            .cloned()
            .unwrap_or_else(|| base.to_string());
        let rate_to_base = bnb_in_base(ctx, &q.reg, base, usdt, wbnb, usdt_dec);
        for path in &cycles {
            // Через налоговые токены цикл не строим (quote/via.rs): все НЕбазовые токены должны быть без налогов.
            if !tax_free(q, &tickers, &base, path) {
                continue;
            }
            let dec = match decimals(q, path).await {
                Ok(d) => d,
                Err(_) => continue,
            };
            let Ok(legs) = cycle::legs_for_cycle(q, path, usdt_dec, &dec) else {
                continue;
            };
            let Ok(p) = cycle::eval(ctx, &legs, input) else {
                continue;
            };
            if let Ok(sig) =
                cycle::build_signal(ctx, base, &base_ticker, &p, &legs, gas, tip, rate_to_base)
            {
                out.push(sig);
            }
        }
    }
    // Прибыльные (порог) впереди, затем по убыванию чистой прибыли; без чистой прибыли — в конец.
    out.sort_by(|a, b| {
        b.profitable(min_profit)
            .cmp(&a.profitable(min_profit))
            .then(net_key(b).cmp(&net_key(a)))
    });
    Ok(out)
}

/// Ключ сортировки по чистой прибыли (None — минимально). Ошибок нет.
fn net_key(s: &CycleSignal) -> Decimal {
    s.net_profit.unwrap_or(Decimal::MIN)
}

/// Чаевые канала отправки в BNB из настроек (решение 41); не заданы — ноль. Ошибок нет.
fn tip_bnb(ctx: &Ctx) -> Decimal {
    super::super::send::planned_tip(ctx)
        .and_then(|w| num::to_decimal(w, num::BNB_DECIMALS))
        .unwrap_or(Decimal::ZERO)
}

/// Курс «базовая монета за 1 BNB» (решение 62: WBNB ≈ BNB). base = WBNB — 1; base = USDT — USDT за 1 BNB из лучшего
/// пула WBNB–USDT (quote/rate.rs); иначе None (перевести нечем — чистой прибыли не будет). Ошибок нет.
fn bnb_in_base(
    ctx: &Ctx,
    reg: &Registry,
    base: Address,
    usdt: Address,
    wbnb: Option<Address>,
    usdt_dec: u8,
) -> Option<Decimal> {
    let wbnb = wbnb?;
    if base == wbnb {
        return Some(Decimal::ONE);
    }
    if base == usdt {
        let one_bnb = U256::from(10u64).pow(U256::from(num::BNB_DECIMALS));
        let units = rate::to_usdt(ctx, reg, usdt, wbnb, one_bnb).ok()?;
        return num::to_decimal(units, usdt_dec);
    }
    None
}

/// Тикеры монет по адресу (из реестра: токен как `coin` своего пула). Ошибок нет.
fn coin_tickers(reg: &Registry) -> HashMap<Address, String> {
    let mut m = HashMap::new();
    for e in reg.all() {
        m.entry(e.coin).or_insert(e.ticker);
    }
    m
}

/// Все НЕбазовые токены цикла — проверенные монеты без налогов (покупки, продажи, перевода): налог мид-свопа официально
/// не описан, не гадаем (quote/via.rs). Нет тикера или проверки, или налог ненулевой — цикл не строим. Ошибок нет.
fn tax_free(
    q: &Quoting,
    tickers: &HashMap<Address, String>,
    base: &Address,
    path: &route_paths::RoutePath,
) -> bool {
    let mut tokens: HashSet<Address> = HashSet::new();
    for s in &path.steps {
        tokens.insert(s.token_in);
        tokens.insert(s.token_out);
    }
    tokens.remove(base);
    tokens.iter().all(|a| {
        let Some(t) = tickers.get(a) else {
            return false;
        };
        matches!(
            q.checks.coin(t).map(|c| c.taxes),
            Some(CheckState::Known(tx))
                if tx.buy_pct.is_zero() && tx.sell_pct.is_zero() && tx.transfer_pct.is_zero()
        )
    })
}

/// decimals всех токенов цикла — из кэша или узла один раз. Ошибки: узел (decimals).
async fn decimals(
    q: &Quoting,
    path: &route_paths::RoutePath,
) -> GatewayResult<HashMap<Address, u8>> {
    let mut set: HashSet<Address> = HashSet::new();
    for s in &path.steps {
        set.insert(s.token_in);
        set.insert(s.token_out);
    }
    let mut m = HashMap::new();
    for t in set {
        m.insert(t, q.tokens.decimals(&q.ctx, t).await?);
    }
    Ok(m)
}
