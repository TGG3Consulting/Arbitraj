//! Файл: core/ledger.rs
//! Что делает: учёт сделок (шаг 6 ТЗ): хранение каждой сделки (план и факт по шагам) в SQLite и в памяти,
//! публикация в интерфейс (экран «Сделки»), деньги в открытых сделках (limits.rs), закрытие сделки —
//! итог, план против факта, итог дня (risk.rs), статистика монеты (gas_stats.rs), уведомление deal.closed,
//! пауза направления (signal.route_cooldown_s). После перезапуска незакрытые сделки помечаются
//! «прервана перезапуском» и ждут проверки человека: состояние ордеров и переводов в памяти потеряно.
//! Кто вызывает: executor.rs, transfers.rs, stuck.rs, manual.rs; engine.rs (load при запуске).
//! От чего зависит: deal.rs, persist.rs, risk.rs, gas_stats.rs, signal.rs, view.rs.
//! Настройки: signal.route_cooldown_s.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::limits::Exposure;
use super::view::CoreEvent;

/// Вид записи в учёте ядра.
const KIND: &str = "deal";

/// Пересчитывает деньги в открытых сделках по всем сделкам в памяти.
fn recompute_exposure(core: &Core) {
    let deals = lk(&core.inner.deals);
    let mut ex = Exposure::default();
    for d in deals.values().filter(|d| d.is_open()) {
        let left = (d.invested - d.proceeds).max(Decimal::ZERO);
        *ex.per_coin.entry(d.direction.asset.clone()).or_default() += left;
        if d.step == Step::Transfer {
            ex.in_transit += left;
        }
    }
    *lk(&core.inner.exposure) = ex;
}

/// Сохраняет сделку (память, SQLite, интерфейс). Ошибка базы — в журнал, сделка продолжается.
pub async fn save(core: &Core, deal: &Deal) {
    lk(&core.inner.deals).insert(deal.id.clone(), deal.clone());
    recompute_exposure(core);
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, &deal.id, deal).await {
        core.inner
            .deps
            .log(
                Level::Error,
                "core.ledger",
                None,
                format!("Сделка {} не сохранена: {e}", deal.id),
                None,
            )
            .await;
    }
    if let Ok(v) = serde_json::to_value(deal) {
        core.inner.live.send(CoreEvent::Deal(v));
    }
}

/// Закрывает сделку: итог, план против факта, итог дня, статистика монеты, уведомление, пауза направления.
/// Принимает: ядро, сделку, конечный шаг (Closed, Halted, TakenOver), причину.
pub async fn finish(core: &Core, deal: &mut Deal, step: Step, why: &str) {
    deal.step = step;
    let result = deal.proceeds - deal.invested - deal.spent;
    if step == Step::Closed {
        deal.result_usd = Some(result);
    }
    deal.note(format!("{step:?}: {why}"));
    save(core, deal).await;
    let deps = &core.inner.deps;
    let d = &deal.direction;
    let text = format!(
        "{} {} → {}: {why}. План {} USDT, факт {} USDT (вложено {}, выручка {}, затраты {})",
        d.asset.ticker,
        d.buy_on.0,
        d.sell_on.0,
        deal.plan_profit_usd.round_dp(4),
        deal.result_usd
            .map(|r| r.round_dp(4).to_string())
            .unwrap_or_else(|| "—".into()),
        deal.invested.round_dp(4),
        deal.proceeds.round_dp(4),
        deal.spent.round_dp(4)
    );
    deps.log(
        Level::Info,
        "core.deal_closed",
        Some(&d.buy_on.0),
        &text,
        serde_json::to_value(&*deal).ok(),
    )
    .await;
    deps.notify("deal.closed", Some(&d.buy_on.0), &text).await;
    if let Some(r) = deal.result_usd {
        super::gas_stats::record_result(core, &d.asset, r).await;
        if let Some(stop) = super::risk::add_result(core, r) {
            deps.notify("risk.daily_loss", None, &stop).await;
        }
    }
    let cooldown = deps
        .settings
        .integer("signal.route_cooldown_s", At::default());
    match cooldown {
        Ok(s) => lk(&core.inner.signals).cooldown(d, s, Utc::now()),
        Err(e) => {
            deps.log(Level::Warn, "core.ledger", None, e.message, None)
                .await
        }
    }
    lk(&core.inner.active).remove(d);
}

/// Загружает сделки при запуске. Незакрытые — «прервана перезапуском», шаг Halted, уведомление.
pub async fn load(core: &Core) {
    let deps = &core.inner.deps;
    let deals = match super::persist::load::<Deal>(&deps.db, KIND).await {
        Ok(d) => d,
        Err(e) => {
            deps.log(
                Level::Error,
                "core.ledger",
                None,
                format!("Сделки не загружены: {e}"),
                None,
            )
            .await;
            return;
        }
    };
    for mut d in deals {
        // Ждущие решения человека остаются ждать (алерты загружает manual.rs).
        if d.is_open() && d.step != Step::WaitingHuman {
            let text = format!(
                "Сделка {} ({}, шаг {:?}, {} монет на {}) прервана перезапуском — проверьте вручную",
                d.id, d.direction.asset.ticker, d.step, d.qty, d.at.0
            );
            d.step = Step::Halted;
            d.note(text.clone());
            deps.log(Level::Warn, "core.ledger", Some(&d.at.0), &text, None)
                .await;
            deps.notify("risk.stop", Some(&d.at.0), &text).await;
            save(core, &d).await;
        } else {
            lk(&core.inner.deals).insert(d.id.clone(), d);
        }
    }
    recompute_exposure(core);
}

impl Core {
    /// Все сделки (экран «Сделки»): идущие и история, план против факта.
    pub fn deals(&self) -> Vec<Deal> {
        let mut v: Vec<Deal> = lk(&self.inner.deals).values().cloned().collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.started_at));
        v
    }
}
