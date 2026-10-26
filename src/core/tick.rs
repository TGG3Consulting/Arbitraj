//! Файл: core/tick.rs
//! Что делает: поток данных одного тика (ARCHITECTURE.md): событие адаптера → market_state →
//! пересчёт только направлений этой монеты, где участвует площадка (2×(N−1)) → signal → «Живой экран»;
//! кандидат — в очередь полного расчёта (candidates.rs); цены — в замеры волатильности (vol.rs).
//! Новые стаканы, пулы и проверки толкают пересборку справочника. Пересчёт одной монеты идёт
//! отдельной задачей; тики, пришедшие во время пересчёта, сливаются в один повтор.
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: engine.rs, market_state.rs, evaluate.rs (пересчёт монеты), view.rs.
//! Настройки: через evaluate.rs — signal.*, money.max_per_trade_usd, profit.sigma_window_min.

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
                if let Some(a) = asset_by_ticker(&core, &ticker) {
                    schedule(&core, a, venue);
                }
            }
            Touched::Pool {
                venue,
                tokens,
                first,
            } => {
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
                if let Some(token) = asset_by_ticker(&core, &ticker).and_then(|a| a.bsc_contract) {
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

/// Монета справочника по тикеру (тикеры приходят от бирж; у DEX — тот, что передан в подписку).
fn asset_by_ticker(core: &Core, ticker: &str) -> Option<Asset> {
    rd(&core.inner.coins)
        .iter()
        .find(|c| c.asset.ticker == ticker && c.asset.bsc_contract.is_some())
        .map(|c| c.asset.clone())
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

/// Ставит пересчёт монеты: если он уже идёт — отмечает повтор, иначе запускает задачу.
fn schedule(core: &Core, asset: Asset, venue: VenueId) {
    // Тик монеты — тем, кто ждёт её цены (режим «Ожидание», stuck.rs).
    let _ = core.inner.coin_ticks.send(asset.clone());
    {
        let mut t = lk(&core.inner.ticking);
        if let Some(again) = t.get_mut(&asset) {
            *again = true;
            return;
        }
        t.insert(asset.clone(), false);
    }
    let core = core.clone();
    tokio::spawn(async move {
        loop {
            super::evaluate::evaluate(&core, &asset, &venue).await;
            let mut t = lk(&core.inner.ticking);
            match t.get_mut(&asset) {
                Some(again) if *again => *again = false,
                _ => {
                    t.remove(&asset);
                    return;
                }
            }
        }
    });
}
