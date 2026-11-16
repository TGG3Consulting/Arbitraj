//! Файл: core/evaluate.rs
//! Что делает: пересчёт направлений одной монеты на тике (поток тика — tick.rs): цены направления (prices.rs) →
//! замеры волатильности (vol.rs) → сигнал (signal.rs) → строка «Живого экрана»; кандидат — в очередь расчёта.
//! Монета с непроверенными налогами (решения 30, 61) — строка «налоги не проверены: причина» без котировок.
//! Сигнал вышел из «кандидата» при открытом эпизоде — эпизод закрывается с причиной (решение 74, episode_run.rs).
//! Решение 317: сигналу передаются обстоятельства пересчёта (signal_ctx.rs — сторона DEX, событие пула); направление
//! стало кандидатом — запись core.signal_hold (signal_hold_log.rs, при log.signal_hold = да). Решение 318: срок
//! дозревания «держится» — в план таймера (signal_ripen.rs); пересчёт, поставленный таймером, помечается «таймер».
//! Решение 322: направления площадки события пула считаются первыми (tick_plan.rs Run.first).
//! Кто вызывает: core/tick_sched.rs.
//! От чего зависит: engine.rs, registry.rs, market_state.rs, prices.rs, signal.rs, signal_ctx.rs, signal_hold_log.rs,
//! vol.rs, candidates.rs, episode_run.rs, view.rs.
//! Настройки: signal.* (через signal.rs), money.max_per_trade_usd (через prices.rs), profit.sigma_window_min,
//! live.spread_min_interval_ms (решение 109), log.signal_hold (через signal_hold_log.rs).

use chrono::Utc;

use crate::core::types::{Asset, Side};

use super::engine::{Core, lk, rd, wr};
use super::signal::{Params, State};
use super::view::CoreEvent;

/// Пересчёт направлений монеты, где участвует любая из площадок тика (решение 109: тики нескольких площадок сливаются
/// в один пересчёт). Принимает: ядро, монету, пересчёт плана (tick_plan.rs Run — площадки; решение 322 — площадки
/// события пула первыми). Ошибок нет: причины — в строках экрана.
pub async fn evaluate(core: &Core, asset: &Asset, run: &super::tick_plan::Run) {
    let venues = &run.venues;
    let (dirs, ref_price, waiting) = {
        // Решение 131: место монеты — из карты пересборки (ticker_index.rs), проход справочника — только если
        // карта ещё от прежнего справочника.
        let coins = rd(&core.inner.coins);
        let Some(entry) = super::ticker_index::find(core, &coins, asset) else {
            return;
        };
        let market = rd(&core.inner.market);
        let ref_price = entry
            .presence
            .iter()
            .filter(|p| p.info.is_some())
            // Решение 211 (ревью В5): стакан биржи — по её тикеру монеты (Presence::ticker).
            .find_map(|p| market.best(&p.venue, &p.ticker, Side::Buy));
        let mut dirs: Vec<_> = entry
            .shown_directions()
            .into_iter()
            .filter(|d| venues.contains(&d.buy_on) || venues.contains(&d.sell_on))
            .collect();
        // Решение 322: направления с площадкой события пула — первыми (сортировка устойчивая; first пуст — порядок
        // прежний).
        if !run.first.is_empty() {
            dirs.sort_by_key(|d| {
                !(run.first.contains(&d.buy_on) || run.first.contains(&d.sell_on))
            });
        }
        // Налоги не проверены (решение 30): строка видна с причиной ожидания, торговли нет.
        let waiting = entry.waits_check().then(|| {
            let why = entry
                .wait
                .clone()
                .unwrap_or_else(|| "проверка ещё не выполнена".into());
            format!("налоги не проверены: {why}")
        });
        (dirs, ref_price, waiting)
    };
    let params = Params::read(&core.inner.deps.settings);
    for dir in dirs {
        // Налоги не проверены — цены не считаются (котировки по таким монетам на каждом тике забивали бы очередь
        // узла): сразу строка с причиной ожидания.
        let prices = match &waiting {
            Some(w) => Err(w.clone()),
            None => super::prices::for_direction(core, &dir, ref_price).await,
        };
        if let Ok(p) = &prices {
            // Замеры цен для σ (vol.rs): покупка на A и продажа на B — отдельные ряды, у каждого направления свои
            // (решение 118: ключ — со площадкой-напарником).
            let now = Utc::now();
            let w = window_min(core);
            let (a, b) = (&dir.buy_on, &dir.sell_on);
            let mut vol = lk(&core.inner.vol);
            vol.sample(a, &dir.asset, Side::Buy, b, p.buy, now, w);
            vol.sample(b, &dir.asset, Side::Sell, a, p.sell, now, w);
        }
        let timer = lk(&core.inner.ripen).take_fired(&dir);
        let now = Utc::now();
        let mut ctx = super::signal_ctx::of(core, &dir, venues, timer);
        // Решение 317 (доработка): сколько стоят уровни стакана биржи на объёме сравнения — из состояния рынка.
        if let Ok(p) = &prices {
            ctx.cex_still_ms = super::signal_ctx::cex_still_ms(core, &dir, &ctx, p.qty, now);
        }
        let mut view = lk(&core.inner.signals).update_with(&dir, prices, &params, now, &ctx);
        if let Some(w) = &waiting {
            view.state = State::TaxesNotChecked(w.clone());
        }
        // Решение 318: срок дозревания (только «держится» при signal.hold_timer = да) или снятие срока. В пересчёт
        // таймер ставит площадку биржи направления (DEX ↔ DEX — площадку A): её тик не путается с событием пула.
        let venue = if ctx.dex_side == Some(crate::core::types::Side::Buy) {
            &dir.sell_on
        } else {
            &dir.buy_on
        };
        lk(&core.inner.ripen).set(&dir, view.ripen_at, venue);
        let (was_anomaly, was_candidate) = rd(&core.inner.spreads)
            .get(&dir)
            .map_or((false, false), |v| {
                (v.state == State::Anomaly, v.state == State::Candidate)
            });
        if view.state == State::Anomaly && !was_anomaly {
            // ТЗ, шаг 3: аномальный спред — перечитать статусы ввода и вывода напрямую из API.
            core.inner.kick_networks.notify_waiters();
        }
        if view.state == State::Candidate {
            if !was_candidate {
                // Решения 317, 318: как и почему направление стало кандидатом (только при log.signal_hold = да).
                super::signal_hold_log::note(core, &view);
            }
            super::candidates::enqueue(core, &dir);
        } else if lk(&core.inner.dry_told).contains(&dir)
            || super::episode_run::is_open(core, &dir)
            || super::window_run::passing(core, &dir)
        {
            // Сигнал вышел из «кандидата» — эпизод закрывается с причиной (решение 74); только память и сообщение
            // задаче эпизодов, тик ничего не ждёт.
            super::episode_run::failed(core, &dir, &format!("сигнал: {}", state_text(&view.state)));
        }
        wr(&core.inner.spreads).insert(dir.clone(), view.clone());
        if live_due(core, &dir, &view.state) {
            core.inner.live.send(CoreEvent::Spread(view));
        }
    }
}

/// Пора ли слать строку «Живого экрана» (решение 109): состояние сменилось или с прошлой отправки по направлению
/// прошло live.spread_min_interval_ms (не задано — всегда). Строка в памяти (spreads) обновляется всегда — REST
/// «Живого экрана» отдаёт свежую. Ошибок нет.
fn live_due(core: &Core, dir: &crate::core::types::Direction, state: &State) -> bool {
    let min = core
        .inner
        .deps
        .settings
        .integer("live.spread_min_interval_ms", Default::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(std::time::Duration::from_millis);
    let mut sent = lk(&core.inner.spread_sent);
    let now = std::time::Instant::now();
    let due = match (sent.get(dir), min) {
        (Some((at, st)), Some(min)) => st != state || now.duration_since(*at) >= min,
        _ => true,
    };
    if due {
        sent.insert(dir.clone(), (now, state.clone()));
    }
    due
}

/// Окно волатильности, минуты (profit.sigma_window_min, решение 49). None — не задано.
pub fn window_min(core: &Core) -> Option<u64> {
    core.inner
        .deps
        .settings
        .integer("profit.sigma_window_min", Default::default())
        .ok()
}

/// Состояние сигнала человеческим текстом — причина закрытия эпизода (решение 74). Ошибок нет.
pub fn state_text(s: &State) -> String {
    match s {
        State::NoPrice(e) => format!("нет цены — {e}"),
        State::NotConfigured(e) => format!("не заданы пороги — {e}"),
        State::Below => "спред не покрывает комиссии".into(),
        State::Holding => "спред держится меньше signal.min_hold_*".into(),
        State::Anomaly => "аномальный спред (signal.anomaly_spread_pct)".into(),
        State::Cooldown { until } => format!("пауза направления до {until}"),
        State::Candidate => "кандидат".into(),
        State::TaxesNotChecked(e) => e.clone(),
    }
}
