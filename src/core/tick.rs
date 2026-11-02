//! Файл: core/tick.rs
//! Что делает: поток данных одного тика (ARCHITECTURE.md): событие адаптера → market_state →
//! пересчёт только направлений этой монеты, где участвует площадка (2×(N−1)) → signal → «Живой экран»;
//! кандидат — в очередь полного расчёта (candidates.rs); цены — в замеры волатильности (vol.rs).
//! Новые стаканы, пулы и проверки толкают пересборку справочника. Пересчёт одной монеты идёт
//! отдельной задачей; тики, пришедшие во время пересчёта, сливаются в один повтор.
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
    while let Some(ev) = rx.recv().await {
        let touched = wr(&core.inner.market).apply(ev);
        match touched {
            Touched::Book {
                venue,
                ticker,
                first,
            } => {
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
                first,
            } => {
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
                for a in assets_by_tokens(&core, tokens) {
                    schedule(&core, a, venue.clone());
                }
            }
            Touched::Invalidated { venue, ticker } => {
                // У DEX данные монеты — её пулы: удаляем их по адресу контракта.
                if let Some(token) =
                    asset_by_ticker(&core, &venue, &ticker).and_then(|a| a.bsc_contract)
                {
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
            Touched::WalletQueue { queue, prev } => {
                super::queue_notify::wallet_queue(&core, queue, prev).await
            }
            Touched::SpotPairs => core.request_rebuild(),
            Touched::PoolsLoading { changed } => {
                if changed {
                    core.request_rebuild();
                }
            }
            Touched::CheckPass {
                venue,
                counts: [ok, failed, waiting, config, loading],
            } => {
                let n = super::first_pass::Counts {
                    ok,
                    failed,
                    waiting,
                    config,
                    loading,
                };
                super::first_pass::done(&core, &venue.0, n).await
            }
            Touched::Balance(b) => super::credit_event::on_change(&core, b).await,
            Touched::Notice { venue, code, text } => {
                core.inner.deps.notify(&code, Some(&venue.0), &text).await
            }
        }
    }
}

/// Монета справочника по площадке и тикеру (тикеры приходят от бирж; у DEX — тот, что передан в подписку).
/// Решение 118 (Я-9): по карте пересборки (ticker_index.rs) — O(1) и только монета этой площадки. Ошибок нет.
fn asset_by_ticker(core: &Core, venue: &VenueId, ticker: &str) -> Option<Asset> {
    rd(&core.inner.tickers).get(venue, ticker)
}

/// Монеты справочника, чей адрес контракта — один из токенов пула.
fn assets_by_tokens(
    core: &Core,
    t: (alloy_primitives::Address, alloy_primitives::Address),
) -> Vec<Asset> {
    rd(&core.inner.coins)
        .iter()
        .filter(|c| c.asset.bsc_contract.is_some_and(|a| a == t.0 || a == t.1))
        .map(|c| c.asset.clone())
        .collect()
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
