//! Файл: core/manual_return.rs
//! Что делает: «Вернуть боту» (решение 37): монеты человека возвращаются в учёт по реальному свободному
//! балансу площадки (не больше взятого), вложенное — пропорционально; дальше — продажа по общим правилам
//! (sell.rs). Плюс данные экрана «Ручное управление» для api.
//! Кто вызывает: core/commands.rs; api (Core::manual).
//! От чего зависит: manual.rs, manual_halted.rs (on_hand — реальный баланс), deal.rs, ledger.rs, sell.rs.
//! Настройки: risk.min_bnb_balance — через native.rs (BNB на DEX — сверх резерва на газ, решение 62).

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
    // Реальный свободный баланс (BNB на DEX — сверх резерва и газа продажи, решение 62) — manual_halted::on_hand.
    let (qty, gas_note) = super::manual_halted::on_hand(core, &h.venue, &h.asset, h.qty).await?;
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
    if let Some(n) = gas_note {
        deal.note(n);
    }
    // Решение 53: доля предварительного убытка «Беру на себя» переходит к сделке «Вернуть боту».
    let mut old = old;
    super::daily::transfer(core, &mut old, &mut deal, share).await;
    super::ledger::save(core, &old).await;
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
    /// Экран «Ручное управление» (GET /api/manual): алерты, монеты человека, остановленные сделки.
    pub fn manual(&self) -> serde_json::Value {
        let alerts: Vec<StuckAlert> = lk(&self.inner.alerts)
            .values()
            .map(|(a, _)| a.clone())
            .collect();
        let holdings: Vec<ManualHolding> = lk(&self.inner.holdings)
            .values()
            .map(|(h, _)| h.clone())
            .collect();
        // Решение 100: остановленные сделки, ждущие разбора («Продать сейчас», «Продано вручную»).
        let halted = self.halted();
        serde_json::json!({ "alerts": alerts, "holdings": holdings, "halted": halted })
    }
}
