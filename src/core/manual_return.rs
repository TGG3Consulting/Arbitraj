//! Файл: core/manual_return.rs
//! Что делает: «Вернуть боту» (решение 37): монеты человека возвращаются в учёт по реальному свободному
//! балансу площадки (не больше взятого), вложенное — пропорционально; дальше — продажа по общим правилам
//! (sell.rs). Плюс данные экрана «Ручное управление» для api.
//! Кто вызывает: core/commands.rs; api (Core::manual).
//! От чего зависит: manual.rs, deal.rs, ledger.rs, sell.rs, deps.rs (шлюз — балансы).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{ManualHolding, StuckAlert};

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// «Вернуть боту»: количество — реальный свободный баланс монеты на площадке (не больше взятого);
/// вложенное — пропорционально. Возвращает: текст. Ошибки: нет записи, баланс не получен или нулевой.
pub async fn return_to_bot(core: &Core, holding_id: &str) -> Result<String, String> {
    let (h, old) = lk(&core.inner.holdings)
        .get(holding_id)
        .cloned()
        .ok_or(format!("записи {holding_id} нет"))?;
    let gw = core
        .inner
        .deps
        .gw
        .get(&h.venue)
        .await
        .map_err(|e| e.message)?;
    let b = gw
        .balances()
        .await
        .map_err(|e| format!("{}: балансы: {}", h.venue.0, e.message))?;
    let free: Decimal = b
        .iter()
        .filter(|x| x.asset == h.asset.ticker)
        .map(|x| x.free)
        .sum();
    if free <= Decimal::ZERO {
        return Err(format!("на {} нет свободных {}", h.venue.0, h.asset.ticker));
    }
    let qty = free.min(h.qty);
    let share = if h.qty.is_zero() {
        Decimal::ZERO
    } else {
        qty / h.qty
    };
    let mut deal = Deal::new(
        format!("{}r", old.id),
        old.direction.clone(),
        old.plan.clone(),
        old.plan_profit_usd,
    );
    deal.at = h.venue.clone();
    deal.qty = qty;
    deal.invested = old.invested * share;
    deal.spent = old.spent * share;
    deal.step = Step::Selling;
    deal.note(format!(
        "«Вернуть боту»: {qty} по реальному балансу (было взято {})",
        h.qty
    ));
    lk(&core.inner.holdings).remove(holding_id);
    super::manual::persist(core).await;
    super::ledger::save(core, &deal).await;
    let text = format!("{qty} {} на {} возвращены боту", h.asset.ticker, h.venue.0);
    core.inner
        .deps
        .notify("manual.returned", Some(&h.venue.0), &text)
        .await;
    let c = core.clone();
    tokio::spawn(async move { super::sell::sell_or_stuck(&c, deal).await });
    Ok(text)
}

impl Core {
    /// Экран «Ручное управление» (GET /api/manual): алерты и монеты человека.
    pub fn manual(&self) -> serde_json::Value {
        let alerts: Vec<StuckAlert> = lk(&self.inner.alerts)
            .values()
            .map(|(a, _)| a.clone())
            .collect();
        let holdings: Vec<ManualHolding> = lk(&self.inner.holdings)
            .values()
            .map(|(h, _)| h.clone())
            .collect();
        serde_json::json!({ "alerts": alerts, "holdings": holdings })
    }
}
