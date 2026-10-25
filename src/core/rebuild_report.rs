//! Файл: core/rebuild_report.rs
//! Что делает: уведомления пересборки справочника — вход и выход монеты из чёрного списка
//! (coin.blacklisted, coin.unblacklisted), итог проверки налогов (coin.check_done), пулы, исключённые
//! по крючку (pool.hook_excluded). Всё — в журнал, интерфейс и Telegram (если событие включено).
//! Чёрный список: журнал и Telegram — только настоящая смена (решение 71, inner.told_blocks); крючки — сводно.
//! Кто вызывает: core/rebuild.rs.
//! От чего зависит: engine.rs, blacklist.rs, registry.rs, view.rs.
//! Настройки: notify.event.* (через notify).

use crate::core::types::BlockReason;
use crate::storage::journal::Level;

use super::blacklist::BlockChange;
use super::engine::{Core, lk};
use super::rebuild::has_pool;
use super::registry::{CoinEntry, Exclusion};
use super::view::CoreEvent;

/// Журнал, интерфейс и Telegram о входе или выходе монеты из чёрного списка. Интерфейс — каждое изменение; журнал и
/// Telegram — только настоящая смена состояния: «ещё не проверена» (NotChecked) — не вход и не выход (в том числе
/// временная, пока пулы недостоверны после обрыва ленты квитанций — решение 71). Выход после NotChecked без прошлого
/// входа и возврат к той же причине после NotChecked — без уведомления; итог проверки — coin.check_done.
pub async fn report_block(core: &Core, ch: BlockChange) {
    let deps = &core.inner.deps;
    let (asset, block) = match ch {
        BlockChange::Same => return,
        BlockChange::Entered(b) => (b.asset.clone(), Some(b)),
        BlockChange::Left(a) => (a, None),
    };
    core.inner.live.send(CoreEvent::CoinBlock {
        asset: asset.clone(),
        block: block.clone(),
    });
    if block
        .as_ref()
        .is_some_and(|b| b.reason == BlockReason::NotChecked)
    {
        return;
    }
    let now = block.as_ref().map(|b| reason_key(&b.reason));
    {
        let mut told = lk(&core.inner.told_blocks);
        if told.get(&asset) == now.as_ref() {
            return;
        }
        match &now {
            Some(k) => told.insert(asset.clone(), k.clone()),
            None => told.remove(&asset),
        };
    }
    let (code, text) = match &block {
        Some(b) => (
            "coin.blacklisted",
            format!("{} — в чёрном списке: {:?}", b.asset.ticker, b.reason),
        ),
        None => (
            "coin.unblacklisted",
            format!("{} вышла из чёрного списка", asset.ticker),
        ),
    };
    deps.log(Level::Info, "core.blacklist", None, &text, None)
        .await;
    // Первый проход после запуска — без уведомления по монете: одно сводное в конце (core/first_pass.rs).
    // Ручной чёрный список — всегда по монете (это действие человека, не итог проверки).
    let manual = matches!(&block, Some(b) if matches!(b.reason, BlockReason::Manual { .. }));
    if manual || !super::first_pass::absorb(core, now) {
        deps.notify(code, None, &text).await;
    }
}

/// Причина для сводки: вид и текст без чисел (адреса и суммы не делают причину новой). Ошибок нет.
fn reason_key(r: &BlockReason) -> String {
    let s = match r {
        BlockReason::CheckFailed { reason } => format!("проверка не прошла: {reason}"),
        other => format!("{other:?}"),
    };
    let s: String = s.chars().filter(|c| !c.is_ascii_digit()).collect();
    s.chars().take(160).collect()
}

/// Уведомляет о пулах, впервые исключённых по крючку (pool.hook_excluded): журнал — по монете, Telegram — одно сводное
/// за пересборку. Ошибок нет.
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
    if fresh.is_empty() {
        return;
    }
    let deps = &core.inner.deps;
    for text in &fresh {
        deps.log(Level::Warn, "core.pool_excluded", None, text, None)
            .await;
    }
    // Telegram — одно сводное за пересборку, а не по пулу (решение 71): сколько монет и первые notify.summary_reasons.
    let shown = match deps.settings.integer(
        "notify.summary_reasons",
        crate::settings::read::At::default(),
    ) {
        Ok(k) => fresh
            .iter()
            .take(k as usize)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("список не показан — {}", e.message),
    };
    let text = format!("Пулы исключены по крючку: монет {}\n{shown}", fresh.len());
    deps.notify("pool.hook_excluded", None, &text).await;
}

/// Итог проверки налогов: когда непроверенных монет больше нет — сколько прошло и сколько
/// в чёрном списке (событие coin.check_done). Ошибок нет.
pub async fn report_check_done(core: &Core, coins: &[CoinEntry]) {
    let waiting = coins
        .iter()
        .filter(|c| matches!(&c.block, Some(b) if b.reason == BlockReason::NotChecked))
        .count();
    let before = std::mem::replace(&mut *super::engine::lk(&core.inner.not_checked), waiting);
    if before == 0 || waiting > 0 || !lk(&core.inner.first_pass).done {
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
