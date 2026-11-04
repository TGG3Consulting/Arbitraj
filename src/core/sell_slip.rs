//! Файл: core/sell_slip.rs
//! Что делает: решение 148 — защита по факту продажи. Сделка закрыта продажей (sell.rs) — факт сравнивается с планом
//! по средней цене продажи: доля = (план − факт) / план × 100, где план — plan.sell_avg сделки (расчёт кандидата,
//! после пересчёта под остаток — новый план), факт — средняя цена всех исполненных продаж сделки на площадке продажи
//! плана (взвешено по количеству: частичные продажи и повторы продажи одной сделки — одно наблюдение). Продажи на другой
//! площадке (продажа обратно на A при частичной покупке, перегон застрявшей монеты) с планом не сравниваются —
//! цена плана к ним не относится; такая сделка счётчик не меняет.
//! По монете (ключ — адрес контракта, coin_key.rs) guard.sell_slip_count продаж ПОДРЯД хуже плана больше чем на
//! guard.sell_slip_pct % — монета уходит в ручной чёрный список тем же механизмом, что ручной (coin.blocked = да,
//! coin.blocked_note = «Авто (решение 148): …»), журнал core.sell_slip_block (Error) и уведомление coin.sell_slip.
//! Продажа не хуже порога — счёт монеты сбрасывается. Уже в ручном списке — отметка и заметка человека не меняются.
//! Счётчик отдельно не хранится: он пересчитывается из учёта сделок в памяти (ledger.rs; при запуске загружается из
//! SQLite — ledger_load.rs), поэтому перезапуск его не обнуляет. Пересчёт идёт по закрытым сделкам монеты в порядке
//! последней продажи; серия, уже дошедшая до порога раньше (тогда монета ушла в список), начинается заново — снятие
//! отметки человеком не блокирует монету снова первой же плохой продажей. Пороги берутся текущие.
//! Кто вызывает: core/sell.rs (после закрытия сделки продажей).
//! От чего зависит: deal.rs, coin_key.rs, blacklist.rs (ручная отметка), engine.rs, deps.rs, settings.
//! Настройки: guard.sell_slip_pct, guard.sell_slip_count (вся система; не задана любая — защита выключена, одна запись в
//! журнал на смену причины); пишет coin.blocked и coin.blocked_note (по монете — тикер, как ручной список).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::coin_key::CoinKey;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: продажи хуже плана (решение 148)";

/// Наблюдение: одна закрытая продажей сделка.
#[derive(Debug, Clone, PartialEq)]
pub struct Obs {
    pub deal: String,
    /// Насколько факт хуже плана, % (больше нуля — хуже, меньше — лучше).
    pub pct: Decimal,
    /// Время последней сравниваемой продажи — порядок серии.
    pub at: DateTime<Utc>,
    /// Итог сделки минусовой (result_usd < 0; Тигран 04.11: в серию идёт только минусовая сделка). Итога нет — false.
    pub loss: bool,
}

/// Наблюдение по сделке. Принимает: сделку. Возвращает: None — сделка не закрыта, нет продаж на площадке продажи
/// плана или в плане нет sell_avg (например, цикл DEX ↔ DEX). Ошибок нет.
pub fn observe(deal: &Deal) -> Option<Obs> {
    if deal.step != Step::Closed {
        return None;
    }
    let plan: Decimal = serde_json::from_value(deal.plan.get("sell_avg")?.clone()).ok()?;
    let sells: Vec<_> = deal
        .sells
        .iter()
        .filter(|s| s.request.venue == deal.direction.sell_on && s.filled_qty > Decimal::ZERO)
        .collect();
    let qty: Decimal = sells.iter().map(|s| s.filled_qty).sum();
    if qty.is_zero() {
        return None;
    }
    let fact = sells
        .iter()
        .map(|s| s.filled_qty * s.avg_price)
        .sum::<Decimal>()
        / qty;
    Some(Obs {
        deal: deal.id.clone(),
        pct: slip_pct(plan, fact)?,
        at: sells.iter().map(|s| s.finished_at).max()?,
        loss: deal.result_usd.is_some_and(|r| r < Decimal::ZERO),
    })
}

/// Насколько факт хуже плана, %: (план − факт) / план × 100. None — план не больше нуля. Ошибок нет.
pub fn slip_pct(plan: Decimal, fact: Decimal) -> Option<Decimal> {
    (plan > Decimal::ZERO).then(|| (plan - fact) / plan * Decimal::ONE_HUNDRED)
}

/// Текущая серия продаж подряд хуже плана больше чем на max_pct с минусовым итогом сделки. Принимает: наблюдения по порядку времени, порог %,
/// сколько подряд. Возвращает: серию; длина ≥ count — порог достигнут последним наблюдением. Серия, дошедшая до
/// count раньше последнего наблюдения, начинается заново (тогда монета ушла в список). Ошибок нет.
pub fn streak(obs: &[Obs], max_pct: Decimal, count: usize) -> Vec<Obs> {
    let mut run: Vec<Obs> = vec![];
    for (i, o) in obs.iter().enumerate() {
        // Тигран 04.11: в серию — продажа хуже плана больше порога И минусовой итог сделки.
        if o.pct > max_pct && o.loss {
            run.push(o.clone());
        } else {
            run.clear();
        }
        if run.len() >= count && i + 1 < obs.len() {
            run.clear();
        }
    }
    run
}

/// Текст заметки чёрного списка: «Авто (решение 148): N продаж подряд хуже плана: −a%, −b% (сделки id…)». Ошибок нет.
pub fn note(run: &[Obs]) -> String {
    let pcts: Vec<String> = run
        .iter()
        .map(|o| format!("−{}%", o.pct.round_dp(2)))
        .collect();
    let ids: Vec<&str> = run.iter().map(|o| o.deal.as_str()).collect();
    format!(
        "Авто (решение 148): {} продаж подряд хуже плана: {} (сделки {})",
        run.len(),
        pcts.join(", "),
        ids.join(", ")
    )
}

/// После закрытия сделки продажей: пересчёт серии монеты и при достижении порога — ручной чёрный список, журнал и
/// уведомление. Принимает: ядро, закрытую сделку. Ошибок нет: настройки не заданы — защита выключена (журнал на смену).
pub async fn after_close(core: &Core, deal: &Deal) {
    let Some(cur) = observe(deal) else { return };
    let Some((max, count)) = limits(core).await else {
        return;
    };
    let key = CoinKey::of(&deal.direction.asset);
    let mut obs: Vec<Obs> = lk(&core.inner.deals)
        .values()
        .filter(|x| CoinKey::of(&x.direction.asset) == key)
        .filter_map(observe)
        .collect();
    obs.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.deal.cmp(&b.deal)));
    let run = streak(&obs, max, count);
    if run.len() < count || run.last().map(|o| &o.deal) != Some(&cur.deal) {
        return;
    }
    block(core, deal, &run).await;
}

/// Пороги защиты. None — не задана любая (одна запись в журнал на смену причины). Ошибок нет.
async fn limits(core: &Core) -> Option<(Decimal, usize)> {
    let s = &core.inner.deps.settings;
    let r = s
        .decimal("guard.sell_slip_pct", At::default())
        .and_then(|p| {
            s.integer("guard.sell_slip_count", At::default())
                .map(|n| (p, n))
        });
    let why = match r {
        Ok((p, n)) if n > 0 => {
            *lk(&core.inner.slip_off) = None;
            return Some((p, n as usize));
        }
        Ok(_) => "guard.sell_slip_count = 0".to_string(),
        Err(e) => e.message,
    };
    let fresh = lk(&core.inner.slip_off).replace(why.clone()) != Some(why.clone());
    if fresh {
        let text = format!("Защита по факту продажи (решение 148) выключена: {why}");
        core.inner
            .deps
            .log(Level::Info, "core.sell_slip_off", None, text, None)
            .await;
    }
    None
}

/// Монета — в ручной чёрный список (если ещё не там), журнал Error и уведомление coin.sell_slip. Ошибок нет.
async fn block(core: &Core, deal: &Deal, run: &[Obs]) {
    let deps = &core.inner.deps;
    let a = &deal.direction.asset;
    let note = note(run);
    let s = &deps.settings;
    let label = super::coin_key::label(a);
    let text = if super::blacklist::manual(s, &a.ticker).is_some() {
        format!(
            "{label}: {note}; монета уже в ручном чёрном списке — отметка и заметка не менялись"
        )
    } else {
        let scope = Scope::Coin(a.ticker.clone());
        let set = async {
            s.set(
                "coin.blocked_note",
                scope.clone(),
                Some(Value::String(note.clone())),
                AUTHOR,
            )
            .await?;
            s.set("coin.blocked", scope, Some(Value::Bool(true)), AUTHOR)
                .await
        };
        match set.await {
            Ok(_) => format!("{label}: в ручном чёрном списке — {note}"),
            Err(e) => {
                format!("{label}: {note}; в чёрный список не внесена ({e}) — внесите вручную")
            }
        }
    };
    let v = &deal.direction.sell_on.0;
    deps.log(Level::Error, "core.sell_slip_block", Some(v), &text, None)
        .await;
    deps.notify("coin.sell_slip", Some(v), &text).await;
}
