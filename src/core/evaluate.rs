//! Файл: core/evaluate.rs
//! Что делает: пересчёт направлений одной монеты на тике (поток тика — tick.rs): цены направления (prices.rs) →
//! замеры волатильности (vol.rs) → сигнал (signal.rs) → строка «Живого экрана»; кандидат — в очередь расчёта.
//! Монета с непроверенными налогами (решения 30, 61) — строка «налоги не проверены: причина» без котировок.
//! Сигнал вышел из «кандидата» при открытом эпизоде — эпизод закрывается с причиной (решение 74, episode_run.rs).
//! Кто вызывает: core/tick.rs.
//! От чего зависит: engine.rs, registry.rs, market_state.rs, prices.rs, signal.rs, vol.rs, candidates.rs, episode_run.rs, view.rs.
//! Настройки: signal.* (через signal.rs), money.max_per_trade_usd (через prices.rs), profit.sigma_window_min,
//! live.spread_min_interval_ms (решение 109).

use chrono::Utc;

use crate::core::types::{Asset, Side, VenueId};

use super::engine::{Core, lk, rd, wr};
use super::signal::{Params, State};
use super::view::CoreEvent;

/// Пересчёт направлений монеты, где участвует любая из площадок тика (решение 109: тики нескольких площадок сливаются
/// в один пересчёт). Ошибок нет: причины — в строках экрана.
pub async fn evaluate(core: &Core, asset: &Asset, venues: &std::collections::HashSet<VenueId>) {
    let (dirs, ref_price, waiting) = {
        let coins = rd(&core.inner.coins);
        let Some(entry) = coins.iter().find(|c| &c.asset == asset) else {
            return;
        };
        let market = rd(&core.inner.market);
        let ref_price = entry
            .presence
            .iter()
            .filter(|p| p.info.is_some())
            .find_map(|p| market.best(&p.venue, &asset.ticker, Side::Buy));
        let dirs: Vec<_> = entry
            .shown_directions()
            .into_iter()
            .filter(|d| venues.contains(&d.buy_on) || venues.contains(&d.sell_on))
            .collect();
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
        let mut view = lk(&core.inner.signals).update(&dir, prices, &params, Utc::now());
        if let Some(w) = &waiting {
            view.state = State::TaxesNotChecked(w.clone());
        }
        let was_anomaly = rd(&core.inner.spreads)
            .get(&dir)
            .is_some_and(|v| v.state == State::Anomaly);
        if view.state == State::Anomaly && !was_anomaly {
            // ТЗ, шаг 3: аномальный спред — перечитать статусы ввода и вывода напрямую из API.
            core.inner.kick_networks.notify_waiters();
        }
        if view.state == State::Candidate {
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
