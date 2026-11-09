//! Файл: core/tick.rs
//! Что делает: поток данных одного тика (ARCHITECTURE.md): событие адаптера → market_state →
//! пересчёт только направлений этой монеты, где участвует площадка (2×(N−1)) → signal → «Живой экран»;
//! кандидат — в очередь полного расчёта (candidates.rs); цены — в замеры волатильности (vol.rs).
//! Новые стаканы, пулы и проверки толкают пересборку справочника. Пересчёт одной монеты идёт
//! отдельной задачей; тики, пришедшие во время пересчёта, сливаются в один повтор. Событие пула толкает поиск циклов
//! DEX ↔ DEX для исполнения (решение 124, cycle_watch.rs). Стакан и пул отмечают живость потока площадки (решение 196,
//! feed_live.rs). Решение 245: событие пула маршрута нашей открытой сделки — отметка задаче точек цены DEX на объём
//! сделки (window_deal.rs touched, только память).
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: engine.rs, market_state.rs, evaluate.rs (пересчёт монеты), view.rs.
//! Настройки: signal.eval_min_interval_ms (решение 109); через evaluate.rs — signal.*, money.max_per_trade_usd,
//! profit.sigma_window_min.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::core::types::{Asset, VenueId};
use crate::gateway::events::MarketEvent;

use super::engine::{Core, lk, rd, wr};
use super::market_state::Touched;
use super::view::CoreEvent;

/// Задача приёма событий адаптеров. Работает, пока жив канал.
pub async fn run(core: Core, mut rx: mpsc::Receiver<MarketEvent>) {
    // Решение 129: события, которые ждут журнал, Telegram или учёт (очередь кошелька, проход проверок, баланс,
    // уведомление площадки), идут своей задачей по порядку — цикл тика их не ждёт и не стоит, пока пишется журнал.
    let (slow_tx, mut slow_rx) = mpsc::unbounded_channel::<Touched>();
    let slow_core = core.clone();
    tokio::spawn(async move {
        while let Some(t) = slow_rx.recv().await {
            super::tick_slow::slow(&slow_core, t).await;
        }
    });
    while let Some(ev) = rx.recv().await {
        let touched = wr(&core.inner.market).apply(ev);
        match touched {
            Touched::Book {
                venue,
                ticker,
                first,
            } => {
                core.inner.feed_seen.mark(&venue);
                if first {
                    core.request_rebuild();
                }
                if let Some(a) = asset_by_ticker(&core, &venue, &ticker) {
                    schedule(&core, a, venue);
                }
            }
            Touched::Pool {
                venue,
                tokens,
                pool,
                first,
            } => {
                core.inner.feed_seen.mark(&venue);
                // Решение 245: пул маршрута нашей сделки — отметка для точки цены DEX на объём сделки (только память).
                super::window_deal::touched(&core, &venue, &pool);
                // Решение 109: пулы токенов изменились — котировки DEX с ними в памяти больше не годятся.
                {
                    let mut c = lk(&core.inner.quote_cache);
                    c.bump(&venue, tokens.0);
                    c.bump(&venue, tokens.1);
                }
                // Пул пришёл — снятые до перечитывания пулы этой монеты вернулись (решение 71).
                {
                    let mut held = super::engine::lk(&core.inner.held);
                    held.remove(&(venue.clone(), tokens.0));
                    held.remove(&(venue.clone(), tokens.1));
                }
                if first {
                    core.request_rebuild();
                }
                // Решение 124: новые состояния пулов — толчок поиска циклов DEX ↔ DEX для исполнения (cycle_watch.rs).
                core.inner.cycles.kick.notify_one();
                for a in assets_by_tokens(&core, tokens) {
                    schedule(&core, a, venue.clone());
                }
            }
            Touched::Invalidated {
                venue,
                ticker,
                address,
            } => {
                // У DEX данные монеты — её пулы: удаляем их по адресу контракта. Решение 211 (ревью В2): адрес из
                // события (DEX) — сразу; нет — монета по (площадка, тикер площадки).
                let token = address.or_else(|| {
                    asset_by_ticker(&core, &venue, &ticker).and_then(|a| a.bsc_contract)
                });
                if let Some(token) = token {
                    lk(&core.inner.quote_cache).bump(&venue, token);
                    wr(&core.inner.market).drop_pools_of(&venue, token);
                    // Пулы сняты до перечитывания, а не исчезли: решение по чёрному списку держится (решение 71).
                    super::engine::lk(&core.inner.held).insert((venue.clone(), token));
                }
                core.request_rebuild();
            }
            Touched::CoinCheck(c) => {
                core.inner.live.send(CoreEvent::CoinCheck(c));
                core.request_rebuild();
            }
            Touched::PoolCheck(c) => {
                core.inner.live.send(CoreEvent::PoolCheck(c));
                core.request_rebuild();
            }
            Touched::Channel(s) => core.inner.live.send(CoreEvent::SendChannel(s)),
            slow @ (Touched::WalletQueue { .. }
            | Touched::CheckPass { .. }
            | Touched::Balance(_)
            | Touched::Notice { .. }) => {
                let _ = slow_tx.send(slow);
            }
            Touched::SpotPairs => core.request_rebuild(),
            Touched::PoolsLoading { changed } => {
                if changed {
                    core.request_rebuild();
                }
            }
        }
    }
}

/// Монета справочника по площадке и ЕЁ тикеру (тикеры приходят от бирж; у DEX — тот, что передан в подписку).
/// Решение 118 (Я-9): по карте пересборки — O(1) и только монета этой площадки; решение 211 — через переводчик
/// (venue_ticker.rs): монета ядра — адрес. Ошибок нет.
fn asset_by_ticker(core: &Core, venue: &VenueId, ticker: &str) -> Option<Asset> {
    super::venue_ticker::asset_by_venue_ticker(core, venue, ticker)
}

/// Монеты справочника, чей адрес контракта — один из токенов пула. Решение 129: по карте адрес → монеты
/// (ticker_index.rs), а не проходом всего справочника на каждое событие пула. Ошибок нет.
fn assets_by_tokens(
    core: &Core,
    t: (alloy_primitives::Address, alloy_primitives::Address),
) -> Vec<Asset> {
    rd(&core.inner.tickers).by_tokens(t)
}

/// Ставит пересчёт монеты: если он уже идёт — запоминает площадку для повтора, иначе запускает задачу.
/// Решение 109: пересчёт одной монеты — не чаще signal.eval_min_interval_ms (не задано — без предела): тики, пришедшие
/// между пересчётами, сливаются в один повтор по всем приславшим площадкам. Так нагрузка зависит от числа монет и
/// этого интервала, а не от того, как часто биржи шлют стаканы (MEXC — снимками потоком).
fn schedule(core: &Core, asset: Asset, venue: VenueId) {
    // Тик монеты — тем, кто ждёт её цены (режим «Ожидание», stuck.rs).
    let _ = core.inner.coin_ticks.send(asset.clone());
    {
        let mut t = lk(&core.inner.ticking);
        if let Some(pending) = t.get_mut(&asset) {
            pending.insert(venue);
            return;
        }
        t.insert(asset.clone(), HashSet::new());
    }
    let core = core.clone();
    tokio::spawn(async move {
        let mut venues: HashSet<VenueId> = [venue].into();
        loop {
            let started = Instant::now();
            super::evaluate::evaluate(&core, &asset, &venues).await;
            if let Some(min) = min_interval(&core) {
                let spent = started.elapsed();
                if spent < min {
                    tokio::time::sleep(min - spent).await;
                }
            }
            let mut t = lk(&core.inner.ticking);
            match t.get_mut(&asset) {
                Some(pending) if !pending.is_empty() => venues = std::mem::take(pending),
                _ => {
                    t.remove(&asset);
                    return;
                }
            }
        }
    });
}

/// Наименьший промежуток между пересчётами одной монеты (signal.eval_min_interval_ms). None — не задан или 0.
fn min_interval(core: &Core) -> Option<Duration> {
    core.inner
        .deps
        .settings
        .integer("signal.eval_min_interval_ms", Default::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
}
