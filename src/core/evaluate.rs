//! Файл: core/evaluate.rs
//! Что делает: пересчёт направлений одной монеты на тике (поток тика — tick.rs): цены направления (prices.rs) →
//! замеры волатильности (vol.rs) → сигнал (signal.rs) → строка «Живого экрана»; кандидат — в очередь расчёта.
//! Монета с непроверенными налогами (решения 30, 61) — строка «налоги не проверены: причина» без котировок.
//! Сигнал вышел из «кандидата» при открытом эпизоде — эпизод закрывается с причиной (решение 74, episode_run.rs).
//! Кто вызывает: core/tick.rs.
//! От чего зависит: engine.rs, registry.rs, market_state.rs, prices.rs, signal.rs, vol.rs, candidates.rs, episode_run.rs, view.rs.
//! Настройки: signal.* (через signal.rs), money.max_per_trade_usd (через prices.rs), profit.sigma_window_min.

use chrono::Utc;

use crate::core::types::{Asset, Side, VenueId};

use super::engine::{Core, lk, rd, wr};
use super::signal::{Params, State};
use super::view::CoreEvent;

/// Пересчёт направлений монеты, где участвует площадка тика. Ошибок нет: причины — в строках экрана.
pub async fn evaluate(core: &Core, asset: &Asset, venue: &VenueId) {
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
            .filter(|d| &d.buy_on == venue || &d.sell_on == venue)
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
            None => super::prices::for_direction(&core.inner.deps, &dir, ref_price).await,
        };
        if let Ok(p) = &prices {
            // Замеры цен для σ (vol.rs): покупка на A и продажа на B — отдельные ряды.
            let now = Utc::now();
            let w = window_min(core);
            let mut vol = lk(&core.inner.vol);
            vol.sample(&dir.buy_on, &dir.asset, Side::Buy, p.buy, now, w);
            vol.sample(&dir.sell_on, &dir.asset, Side::Sell, p.sell, now, w);
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
        } else if lk(&core.inner.dry_told).remove(&dir) | super::episode_run::is_open(core, &dir) {
            // Сигнал вышел из «кандидата» — эпизод закрывается с причиной (решение 74).
            super::episode_run::failed(core, &dir, &format!("сигнал: {}", state_text(&view.state)))
                .await;
        }
        wr(&core.inner.spreads).insert(dir.clone(), view.clone());
        core.inner.live.send(CoreEvent::Spread(view));
    }
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
