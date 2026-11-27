//! Файл: venues/bsc_dex/quote/cycle_scan.rs
//! Что делает: поиск замкнутых циклов DEX ↔ DEX по всем базовым монетам кошелька и сбор их сигналов (решение 85, фаза
//! сигнала — без исполнения). За переключателем направления dex_dex.enabled (решение 85: каждое направление включается
//! отдельно): не задано/выключено — поиск не работает (пусто, как везде в проекте). Строит тот же граф пулов всех DEX
//! (pools/graph.rs, решение 80), для каждой базовой монеты (dex_dex.base_tokens) перечисляет циклы base → … → base
//! и отбирает dex_dex.max_cycles лучших по курсу круга (quote/cycle_rank.rs, решение 404), считает каждый цикл
//! (quote/cycle.rs) на ОПТИМАЛЬНОМ входе в пределах dex_dex.probe_input_base (верхняя граница; quote/cycle_opt.rs,
//! решение 124) и собирает сигналы — вход и прибыль сигнала — на этом входе. Оба обхода цикла — отдельными
//! сигналами (разные первые рёбра), считаются независимо (решение 85).
//! Через налоговые токены цикл не строим (как мост через промежуточную — quote/via.rs): каждый НЕбазовый токен цикла
//! должен быть проверенной монетой без налогов (покупки/продажи/перевода); иначе цикл пропускается — «налог токена на
//! каждом свопе» (решение 85) тогда структурно равен нулю, не гадаем о неописанной механике мид-свопного налога.
//! Газ свопов и чаевые (BNB) переводятся в базовую монету курсом из графа (решение 62: WBNB ≈ BNB; для базовой USDT —
//! лучший пул WBNB–USDT, quote/rate.rs). Нет перевода — чистой прибыли нет, видна валовая (quote/cycle.rs). Чаевые — только
//! если цикл пойдёт не бандлом свопов (exec/cycle_gate.rs form): бандл их не шлёт (Тигран 09.11: согласовано).
//! Первичный фильтр BingX ↔ DEX (монета на бирже / выводится по BSC) к DEX ↔ DEX НЕ применяется (решение 85): нужна
//! только ликвидность пулов и совпадение адресов токенов в пулах (это обеспечивает граф и реестр).
//! Сам поиск сделок не запускает: сигналы забирает ядро (core/cycle_watch.rs, решение 124) и интерфейс.
//! Решение 308: при dex.graph_background = да граф — снимок фонового владельца, пути циклов гидратируются
//! (route_cache::hydrate — состояние пулов свежее из реестра, а не копии из графа).
//! Решение 336: расчёт найденных циклов (ноги, оптимальный вход, форма, сигнал) — на dex.graph_threads потоках
//! (quote/par.rs), итоги по порядку путей — тот же список; брошенный поиск (quote/abandon.rs) останавливает помощников.
//! Кто вызывает: venue.rs (dex_cycles — ядро и «Живой экран»); тесты — напрямую quote/cycle.rs.
//! От чего зависит: quote/cycle.rs, quote/route_paths.rs, quote/rate.rs, pools/graph.rs, pools/registry.rs, tokens.rs,
//! send/mod.rs (чаевые канала), exec/cycle_gate.rs (форма исполнения), num.rs.
//! Решение 373: цикл со строгим верхом отдачи ρ < 1 (quote/bound_cycle.rs) не считается — сигнала нет, счётчик в сводке.
//! Решение 385: доля sieve.audit_share отсечённых (quote/cycle_audit.rs Audit — один на проход) считается полностью тем
//! же путём (сигнал — как у любого цикла) и сверяется с ρ тех же ног; путь с записанным нарушением не отсекается.
//! Решение 404: к полному расчёту идут dex_dex.max_cycles ЛУЧШИХ циклов базовой по курсу круга (quote/cycle_rank.rs —
//! перебор всех циклов снимка графа в пределах dex_dex.max_cycle_len и необязательного dex_dex.rank_max_cycles), а не
//! первые найденные. Каждый отпавший отобранный цикл учитывается с причиной (quote/cycle_tally.rs Lost), итог прохода по
//! каждой базовой — для журнала ядра (Venue::dex_cycle_tally → core/cycle_scan_log.rs). Налоговый фильтр (tax_free)
//! принимает USDT по его проверке налогов (check/service_tax.rs): раньше USDT не был «монетой подписки» (coin_tickers) и
//! любой цикл базы WBNB через USDT выбрасывался молча.
//! Настройки: dex_dex.enabled, dex_dex.base_tokens, dex_dex.max_cycle_len, dex_dex.max_cycles, dex_dex.rank_max_cycles
//! (решение 404, необязательная), dex_dex.probe_input_base,
//! dex_dex.min_profit_base, dex.graph_threads (решение 336) [bsc_dex]; size.refine_steps (шаги поиска входа); через граф — dex.route_max_pools_per_edge и глубина (решение 73).

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::CheckState;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::cycle_gate::{self, Form};
use super::super::num;
use super::super::pools::graph_read;
use super::super::pools::registry::Registry;
use super::Quoting;
use super::cycle::{self, CycleSignal};
use super::cycle_audit::{Audit, Gate};
use super::cycle_tally::{Lost, Tally};
use super::{cycle_opt, cycle_rank, cycle_rate, rate, route_cache, route_paths};

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

/// Решение 404: предел просмотра циклов при отборе лучших (dex_dex.rank_max_cycles, необязательная): не задана или 0 —
/// без предела (все циклы в пределах dex_dex.max_cycle_len). Ошибок нет.
pub fn rank_cap(ctx: &Ctx) -> Option<u64> {
    ctx.int("dex_dex.rank_max_cycles").ok().filter(|n| *n > 0)
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
    let cap = rank_cap(ctx);
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
    let mut bases = ctx
        .settings
        .addresses("dex_dex.base_tokens", ctx.at())
        .map_err(|e| ctx.tag(e))?;
    if bases.is_empty() {
        return Err(ctx.not_set(
            "dex_dex.base_tokens",
            "dex_dex.enabled = да, а список dex_dex.base_tokens пуст",
        ));
    }
    let cash = match super::cycle_cash::configured(q) {
        Ok(Some(p)) if p.token != usdt => super::cycle_cash::checked(q, p.token)
            .await
            .ok()
            .flatten()
            .map(|_| p),
        _ => None,
    };
    if let Some(c) = cash
        && c.token != usdt
        && bases.contains(&usdt)
        && !bases.contains(&c.token)
    {
        bases.push(c.token);
    }
    // Решение 131: граф — тот же кэш на состояние реестра, что у котировки (pools/graph.rs cached), а не новое
    // построение на каждый толчок поиска (каждое событие пула). Решение 301 (е): блок в ключ графа не входит.
    // Повторное ревью (д): режим читается один раз — снимок фонового графа всегда идёт с гидратацией.
    let (g, hydrated) = graph_read::graph_for(ctx, &q.reg, &q.tokens, usdt_dec)?;
    let wbnb = ctx.addr("dex.native_wrapped").ok();
    let tickers = coin_tickers(&q.reg);
    let tip = tip_bnb(ctx);
    // Решение 385: контроль отсева — один на проход (доля и множество исключённых путей читаются один раз).
    let audit = Audit::start(ctx);
    // Решение 336: при dex.graph_threads > 1 сначала — все пути циклов (decimals — из кэша или узла, по очереди), затем
    // расчёт каждого цикла (ноги, оптимальный вход, форма, сигнал — чистый CPU, независимо) — на нескольких потоках
    // (quote/par.rs), итоги по порядку путей. Не задана — как до 336: расчёт сразу за decimals своего цикла (ревью Б1).
    let want = super::par::want(ctx, "dex.graph_threads");
    let mut out: Vec<CycleSignal> = Vec::new();
    let mut jobs: Vec<Job> = Vec::new();
    // Решение 404: курсы рёбер — одна память на проход (на снимок графа), общая для всех базовых; без замков.
    // Ревью t189: стороны пулов по книге глубины — одним чтением на проход (quote/cycle_rate.rs).
    let mut rates = cycle_rate::Rates::new(&g, q.reg.depth());
    let mut tallies: Vec<Tally> = Vec::new();
    for base in bases {
        let started = std::time::Instant::now();
        let physical = cash.is_some_and(|p| p.token == base) && usdt != base;
        let ranked = if physical {
            cycle_rank::top_settlement(
                ctx,
                &g,
                &mut rates,
                base,
                max_len,
                max_cycles,
                cap,
                Some(usdt),
            )
        } else {
            cycle_rank::top(ctx, &g, &mut rates, base, max_len, max_cycles, cap)
        };
        let rank_ms = started.elapsed().as_millis() as u64;
        let base_ticker = if physical {
            "USDT".to_owned()
        } else {
            tickers
                .get(&base)
                .cloned()
                .unwrap_or_else(|| base.to_string())
        };
        let ti = tallies.len();
        tallies.push(Tally::new(&base_ticker, &ranked, rank_ms));
        let cycles = ranked.paths;
        // Решение 308: в режиме «граф без ожидания» состояние пулов цикла — свежее из реестра (снимок — топология).
        let cycles = if hydrated {
            let picked = cycles.len();
            let fresh = route_cache::hydrate(&q.reg, &cycles);
            // Решение 404: путь, чей пул пропал из реестра или недостоверен, гидратация выбрасывает — в итог прохода.
            tallies[ti].stale(picked.saturating_sub(fresh.len()) as u64);
            fresh
        } else {
            cycles
        };
        let rate_to_base = if physical {
            None
        } else {
            bnb_in_base(ctx, &q.reg, base, usdt, wbnb, usdt_dec)
        };
        for path in cycles {
            // Через налоговые токены цикл не строим (quote/via.rs): все НЕбазовые токены должны быть без налогов.
            if let Err(why) = tax_free(q, &tickers, &base, usdt, &path) {
                tallies[ti].lost(Lost::Tax(why));
                continue;
            }
            let dec = match decimals(q, &path).await {
                Ok(d) => d,
                Err(e) => {
                    tallies[ti].lost(Lost::Decimals(e.message));
                    continue;
                }
            };
            if want <= 1 {
                let b = (
                    base,
                    base_ticker.as_str(),
                    rate_to_base,
                    physical.then_some(usdt),
                );
                let r = signal(q, &audit, b, &path, &dec, usdt_dec, input, gas, tip);
                take(&mut tallies[ti], &mut out, r);
                continue;
            }
            jobs.push(Job {
                tally: ti,
                base,
                base_ticker: base_ticker.clone(),
                rate_to_base,
                physical_cash: physical.then_some(usdt),
                path,
                dec,
            });
        }
    }
    if want > 1 {
        let one = |j: &Job| {
            let b = (
                j.base,
                j.base_ticker.as_str(),
                j.rate_to_base,
                j.physical_cash,
            );
            signal(q, &audit, b, &j.path, &j.dec, usdt_dec, input, gas, tip)
        };
        let alive = || !super::abandon::is_set();
        let Some(found) = super::par::map(ctx, want, &jobs, one, &alive) else {
            super::abandon::check(ctx, "поиск циклов DEX ↔ DEX")?;
            return Ok(Vec::new());
        };
        // Итоги по порядку путей (quote/par.rs) — учёт по своей базовой, как при проходе по очереди.
        for (j, r) in jobs.iter().zip(found) {
            take(&mut tallies[j.tally], &mut out, r);
        }
    }
    // Решение 404: итог прохода по каждой базовой — для журнала ядра (одна запись на проход).
    super::cycle_tally::remember(&ctx.venue().0, tallies.iter().map(Tally::line).collect());
    // Прибыльные (порог) впереди, затем по убыванию чистой прибыли; без чистой прибыли — в конец.
    out.sort_by(|a, b| {
        b.profitable(min_profit)
            .cmp(&a.profitable(min_profit))
            .then(net_key(b).cmp(&net_key(a)))
    });
    Ok(out)
}

/// Решение 404: итог расчёта одного цикла — в сигналы или в учёт потерь своей базовой. Ошибок нет.
fn take(t: &mut Tally, out: &mut Vec<CycleSignal>, r: Result<CycleSignal, Lost>) {
    match r {
        Ok(s) => {
            t.signal(&s);
            out.push(s);
        }
        Err(l) => t.lost(l),
    }
}

/// Цикл к расчёту: номер итога его базовой (решение 404), базовая монета, её тикер и курс BNB в ней, путь, decimals
/// его токенов.
struct Job {
    tally: usize,
    base: Address,
    base_ticker: String,
    rate_to_base: Option<Decimal>,
    physical_cash: Option<Address>,
    path: route_paths::RoutePath,
    dec: HashMap<Address, u8>,
}

/// Сигнал одного цикла (было телом цикла scan): ноги, оптимальный вход (решение 124), форма исполнения, сигнал.
/// Только чтение — можно считать параллельно (решение 336). Принимает: окружение котировки, контроль отсева прохода
/// (решение 385), (базовая, её тикер, курс
/// BNB в ней), путь, decimals его токенов, decimals USDT, верхнюю границу входа, цену газа, чаевые (BNB). Возвращает:
/// сигнал; Err — почему цикл не стал сигналом (решение 404: причина в итог прохода, раньше — молча). Ошибок нет.
#[allow(clippy::too_many_arguments)]
fn signal(
    q: &Quoting,
    audit: &Audit,
    (base, base_ticker, rate_to_base, working): (Address, &str, Option<Decimal>, Option<Address>),
    path: &route_paths::RoutePath,
    dec: &HashMap<Address, u8>,
    usdt_dec: u8,
    input: Decimal,
    gas: U256,
    tip: Decimal,
) -> Result<CycleSignal, Lost> {
    let ctx = &q.ctx;
    let legs = cycle::legs_for_cycle(q, path, usdt_dec, dec).map_err(Lost::Legs)?;
    // Решение 373: строгий верх отдачи цикла ρ < 1 (quote/bound_cycle.rs) ⇒ выход < вход на любом входе ⇒ валовая
    // и чистая прибыль < 0 ≤ dex_dex.min_profit_base — оптимальный вход не ищется (сигнала нет, счётчик в сводке
    // dex.quote_count). Порог < 0 или верх не строгий (вид пула, крючок) — расчёт как раньше.
    // Решение 385: отсечённый, не выбранный контролем, — не считается; выбранный — полный счёт и сверка с ρ.
    let rho = match audit.gate(ctx, &legs) {
        Gate::Cut => return Err(Lost::Cut),
        Gate::Calc(rho) => rho,
    };
    // Решение 124: оптимальный вход на (0, dex_dex.probe_input_base] (quote/cycle_opt.rs), а не фиксированная проба.
    let p = cycle_opt::best(ctx, &legs, input).map_err(|e| Lost::Opt(e.message))?;
    // Тигран 09.11 (согласовано, план = факт): цикл, который пойдёт бандлом свопов (exec/cycle_gate.rs form — тот же
    // выбор, что у сделки), чаевых не шлёт — в сигнале их нет. Исполнитель или форма не выбрана — как было.
    let tip = match cycle_gate::form(ctx, &legs) {
        Ok(Form::Bundle) => Decimal::ZERO,
        _ => tip,
    };
    let units = p.hops.iter().map(|h| h.usable.gas).sum();
    let extra = q
        .gas
        .extra(ctx, units, 1)
        .map_err(|e| Lost::Build(e.message))?;
    let mut sig = cycle::build_signal_with_fees(
        ctx,
        base,
        base_ticker,
        &p,
        &legs,
        gas,
        extra,
        tip,
        rate_to_base,
    )
    .map_err(|e| Lost::Build(e.message))?;
    if let Some(working) = working {
        let wei = super::direct::ceil_units(sig.bnb_cost, super::super::num::BNB_DECIMALS);
        sig.bnb_cost_in_base =
            wei.and_then(|w| super::cycle_cash::replacement(q, base, working, w).ok());
        sig.net_profit = sig.bnb_cost_in_base.map(|cost| sig.gross_profit - cost);
    }
    if let Some(r) = rho {
        audit.verify(r, &sig, &legs);
    }
    Ok(sig)
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
    for e in reg.snapshot() {
        m.entry(e.coin).or_insert_with(|| e.ticker.clone());
    }
    m
}

/// Все НЕбазовые токены цикла — проверенные монеты без налогов (покупки, продажи, перевода): налог мид-свопа официально
/// не описан, не гадаем (quote/via.rs). Нет тикера или проверки, или налог ненулевой — цикл не строим. Решение 404:
/// USDT — служебная монета, «монетой подписки» не бывает (coin_tickers), его налоги проверяет check/service_tax.rs под
/// его адресом — берётся по проверке, без тикера (раньше любой цикл через USDT, то есть база WBNB → … → USDT → WBNB,
/// выбрасывался). Принимает: котировку, тикеры, базовую, адрес USDT, путь. Возвращает: Err — какой токен и почему.
fn tax_free(
    q: &Quoting,
    tickers: &HashMap<Address, String>,
    base: &Address,
    usdt: Address,
    path: &route_paths::RoutePath,
) -> Result<(), String> {
    let mut tokens: HashSet<Address> = HashSet::new();
    for s in &path.steps {
        tokens.insert(s.token_in);
        tokens.insert(s.token_out);
    }
    tokens.remove(base);
    let mut list: Vec<Address> = tokens.into_iter().collect();
    // Порядок — по адресу: причина в итоге прохода одна и та же при том же цикле.
    list.sort();
    for a in list {
        // Решение 211: налоги — по адресу; тикер нужен только чтобы токен был монетой подписки (кроме USDT, решение 404).
        if a != usdt && !tickers.contains_key(&a) {
            return Err(format!("токен {a:#x} не монета подписки"));
        }
        match q.checks.coin(&a).map(|c| c.taxes) {
            Some(CheckState::Known(tx))
                if tx.buy_pct.is_zero() && tx.sell_pct.is_zero() && tx.transfer_pct.is_zero() => {}
            Some(CheckState::Known(_)) => return Err(format!("токен {a:#x}: налог не 0")),
            _ => return Err(format!("токен {a:#x}: налоги не проверены")),
        }
    }
    Ok(())
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
