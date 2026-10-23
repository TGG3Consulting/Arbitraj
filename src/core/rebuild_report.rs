//! Файл: core/rebuild_report.rs
//! Что делает: уведомления пересборки справочника — вход и выход монеты из чёрного списка
//! (coin.blacklisted, coin.unblacklisted), итог проверки налогов (coin.check_done), пулы, исключённые
//! по крючку (pool.hook_excluded). Всё — в журнал, интерфейс и Telegram (если событие включено).
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: engine.rs, blacklist.rs, registry.rs, view.rs.
//! Настройки: notify.event.* (через notify).

use crate::core::types::BlockReason;
use crate::storage::journal::Level;

use super::blacklist::BlockChange;
use super::engine::Core;
use super::rebuild::has_pool;
use super::registry::{CoinEntry, Exclusion};
use super::view::CoreEvent;

/// Журнал, интерфейс и Telegram о входе или выходе монеты из чёрного списка.
pub async fn report_block(core: &Core, ch: BlockChange) {
    let deps = &core.inner.deps;
    let (code, asset, block, text) = match ch {
        BlockChange::Same => return,
        // «Ещё не проверена» — обычное состояние при старте: в интерфейс, без журнала и Telegram;
        // итог проверки — событие coin.check_done (report_check_done).
        BlockChange::Entered(b) if b.reason == BlockReason::NotChecked => {
            core.inner.live.send(CoreEvent::CoinBlock {
                asset: b.asset.clone(),
                block: Some(b),
            });
            return;
        }
        BlockChange::Entered(b) => (
            "coin.blacklisted",
            b.asset.clone(),
            Some(b.clone()),
            format!("{} — в чёрном списке: {:?}", b.asset.ticker, b.reason),
        ),
        BlockChange::Left(a) => (
            "coin.unblacklisted",
            a.clone(),
            None,
            format!("{} вышла из чёрного списка", a.ticker),
        ),
    };
    deps.log(Level::Info, "core.blacklist", None, &text, None)
        .await;
    deps.notify(code, None, &text).await;
    core.inner.live.send(CoreEvent::CoinBlock { asset, block });
}

/// Уведомляет о пулах, впервые исключённых по крючку (pool.hook_excluded). Ошибок нет.
pub async fn report_pools(core: &Core, coins: &[CoinEntry]) {
    let mut fresh = vec![];
    {
        let mut seen = super::engine::lk(&core.inner.excluded_pools);
        for c in coins {
            for p in &c.presence {
                for e in &p.excluded {
                    if let Exclusion::PoolsExcluded(reasons) = e
                        && seen.insert((p.venue.clone(), c.asset.ticker.clone()))
                    {
                        fresh.push(format!(
                            "{} {}: {}",
                            p.venue.0,
                            c.asset.ticker,
                            reasons.join("; ")
                        ));
                    }
                }
            }
        }
    }
    for text in fresh {
        let deps = &core.inner.deps;
        deps.log(Level::Warn, "core.pool_excluded", None, &text, None)
            .await;
        deps.notify("pool.hook_excluded", None, &text).await;
    }
}

/// Итог проверки налогов: когда непроверенных монет больше нет — сколько прошло и сколько
/// в чёрном списке (событие coin.check_done). Ошибок нет.
pub async fn report_check_done(core: &Core, coins: &[CoinEntry]) {
    let waiting = coins
        .iter()
        .filter(|c| matches!(&c.block, Some(b) if b.reason == BlockReason::NotChecked))
        .count();
    let before = std::mem::replace(&mut *super::engine::lk(&core.inner.not_checked), waiting);
    if before == 0 || waiting > 0 {
        return;
    }
    let on_dex = |c: &&CoinEntry| {
        c.presence
            .iter()
            .any(|p| p.info.is_none() && has_pool(&p.excluded))
    };
    let blocked = coins
        .iter()
        .filter(on_dex)
        .filter(|c| c.block.is_some())
        .count();
    let passed = coins.iter().filter(on_dex).count() - blocked;
    let text = format!("Проверка налогов завершена: прошли {passed}, в чёрном списке {blocked}");
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.check_done", None, &text, None)
        .await;
    deps.notify("coin.check_done", None, &text).await;
}
