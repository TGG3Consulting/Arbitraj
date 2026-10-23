//! Файл: core/tick.rs
//! Что делает: поток данных одного тика (ARCHITECTURE.md): событие адаптера → market_state →
//! пересчёт только направлений этой монеты, где участвует площадка (2×(N−1)) → signal → «Живой экран»;
//! кандидат — в очередь полного расчёта (candidates.rs); цены — в замеры волатильности (vol.rs).
//! Новые стаканы, пулы и проверки толкают пересборку справочника. Пересчёт одной монеты идёт
//! отдельной задачей; тики, пришедшие во время пересчёта, сливаются в один повтор.
//! Кто вызывает: core/engine.rs (задача run).
//! От чего зависит: engine.rs, market_state.rs, prices.rs, signal.rs, view.rs.
//! Настройки: signal.* (через signal.rs), money.max_per_trade_usd (через prices.rs).

use chrono::Utc;
use tokio::sync::mpsc;

use crate::core::types::{Asset, Side, VenueId};
use crate::gateway::events::MarketEvent;

use super::engine::{Core, lk, rd, wr};
use super::market_state::Touched;
use super::signal::{Params, State};
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
                block,
                at,
            } => {
                lk(&core.inner.clock).sample(block, at);
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
            Touched::WalletQueue(q) => wallet_queue(&core, q).await,
        }
    }
}

/// Очередь кошелька DEX: остановлена до ручной проверки или снята — уведомление (send.queue_halted /
/// send.queue_released). Занята / свободна — без уведомления. Ошибок нет.
async fn wallet_queue(core: &Core, q: crate::core::types::WalletQueue) {
    use crate::core::types::WalletQueueState as S;
    let venue = q.venue.0.clone();
    let (code, text) = match &q.state {
        S::Halted { reason, .. } => (
            "send.queue_halted",
            format!("{venue}: отправки остановлены до ручной проверки — {reason}"),
        ),
        S::Free => (
            "send.queue_released",
            format!("{venue}: очередь кошелька свободна"),
        ),
        S::Busy { .. } => return,
    };
    core.inner.deps.notify(code, Some(&venue), &text).await;
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
            evaluate(&core, &asset, &venue).await;
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

/// Пересчёт направлений монеты, где участвует площадка тика. Ошибок нет: причины — в строках экрана.
async fn evaluate(core: &Core, asset: &Asset, venue: &VenueId) {
    let (dirs, ref_price) = {
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
            .directions()
            .into_iter()
            .filter(|d| &d.buy_on == venue || &d.sell_on == venue)
            .collect();
        (dirs, ref_price)
    };
    let params = Params::read(&core.inner.deps.settings);
    for dir in dirs {
        let prices = super::prices::for_direction(&core.inner.deps, &dir, ref_price).await;
        if let Ok(p) = &prices {
            // Замеры цен для σ (vol.rs): покупка на A и продажа на B — отдельные ряды.
            let now = Utc::now();
            let mut vol = lk(&core.inner.vol);
            vol.sample(&dir.buy_on, &dir.asset, Side::Buy, p.buy, now);
            vol.sample(&dir.sell_on, &dir.asset, Side::Sell, p.sell, now);
        }
        let view = lk(&core.inner.signals).update(&dir, prices, &params, Utc::now());
        let was_anomaly = rd(&core.inner.spreads)
            .get(&dir)
            .is_some_and(|v| v.state == State::Anomaly);
        if view.state == State::Anomaly && !was_anomaly {
            // ТЗ, шаг 3: аномальный спред — перечитать статусы ввода и вывода напрямую из API.
            core.inner.kick_networks.notify_waiters();
        }
        if view.state == State::Candidate {
            super::candidates::enqueue(core, &dir);
        }
        wr(&core.inner.spreads).insert(dir.clone(), view.clone());
        core.inner.live.send(CoreEvent::Spread(view));
    }
}
