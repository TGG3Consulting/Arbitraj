//! Файл: core/exit_gap.rs
//! Что делает: решение 248 (техлид 10.11 по поручению Тиграна, кейс SMARS 10.11: монета 0xc0366a10…, mexc → bsc_dex,
//! две сделки за 10 часов с одним и тем же откатом продажи «мало на выходе» — налог продажи в сети выше модельного) —
//! бот учится на таком отказе. Адаптер DEX после отката продажи выхода «мало на выходе» замеряет реальный выход той
//! же продажи (симуляция с минимумом 1 ед. с реального баланса кошелька — exec/executor/remeasure.rs,
//! exec/ur_remeasure.rs) и шлёт MarketEvent::ExitGap. Здесь: замер ниже модели больше чем на guard.exit_gap_pct % —
//! монета закрывается для НОВЫХ входов тем же механизмом ручного чёрного списка (coin.blocked = да, coin.blocked_note =
//! «Авто (решение 248): …»), налог по замеру — в coin.exit_tax_seen_pct, журнал core.exit_gap и уведомление
//! coin.exit_gap. Идущие сделки не трогаются (их продажа — выход из позиции, чёрный список её не держит).
//! Снятие: свежая проверка налогов монеты (CoinCheck — в том числе внеочередная перепроверка, решение 247 п. Б)
//! дала налог продажи, при котором модельный выход сходится с замером в пределах guard.exit_gap_pct, — отметка,
//! заметка и налог по замеру снимаются сами (журнал и уведомление); или человек снимает отметку на экране «Монеты».
//! Уже в ручном списке (человек или другое авто-правило) — отметка и заметка не меняются, только журнал и уведомление.
//! Кто вызывает: core/tick_slow.rs (Touched::ExitGap, Touched::CoinCheck).
//! От чего зависит: blacklist.rs (ручная отметка, область монеты), engine.rs, deps.rs, settings.
//! Настройки: guard.exit_gap_pct (вся система, необязательная; не задана — авто-закрытия нет, запись в журнал);
//! пишет coin.blocked, coin.blocked_note, coin.exit_tax_seen_pct (по монете — по адресу, решение 211).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{CheckState, CoinCheck, ExitGap};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;

/// Кто меняет настройку (журнал изменений настроек).
const AUTHOR: &str = "ядро: реальный выход продажи ниже модели (решение 248)";
/// Начало заметки авто-закрытия — по нему снимается только своя отметка (не ручная и не другого правила).
pub const NOTE_HEAD: &str = "Авто (решение 248)";
/// Код уведомления (notify/catalog.rs).
const CODE: &str = "coin.exit_gap";

/// Насколько замер ниже модели, % от модели. None — модель не больше нуля. Ошибок нет.
pub fn gap_pct(model: Decimal, real: Decimal) -> Option<Decimal> {
    (model > Decimal::ZERO).then(|| (model - real) / model * Decimal::ONE_HUNDRED)
}

/// Налог продажи по замеру, %: 100 − (100 − t) × замер / модель — НИЖНЯЯ граница налога в сети (выход пула вогнут
/// по входу: меньший вход даёт не меньше пропорционального выхода, — так же, как exec/executor/remeasure.rs seen_out).
/// Замер не ниже модели — налог модели. Принимает: модель, замер (USDT), модельный налог %. Ошибок нет.
pub fn seen_tax_pct(model: Decimal, real: Decimal, t: Decimal) -> Decimal {
    let h = Decimal::ONE_HUNDRED;
    if model <= Decimal::ZERO || real >= model {
        return t;
    }
    h - (h - t) * real / model
}

/// Сходится ли налог новой проверки с замером: модельный выход при налоге `t_new` (пропорционально входу) выше
/// замера не больше чем на `max` % — 1 − (100 − X) / (100 − t_new) ≤ max / 100. Принимает: налог по замеру X, новый
/// налог продажи, допуск %. Налог ≥ 100 % — сходится (выхода нет ни по модели, ни по замеру). Ошибок нет.
pub fn matches(seen: Decimal, t_new: Decimal, max: Decimal) -> bool {
    let h = Decimal::ONE_HUNDRED;
    if t_new >= h {
        return true;
    }
    Decimal::ONE - (h - seen) / (h - t_new) <= max / h
}

/// Замер выхода продажи: ниже модели больше guard.exit_gap_pct — монета закрыта для входа. Ошибок нет (сбой записи —
/// текст в журнал и уведомление: «внесите вручную»).
pub async fn on_gap(core: &Core, g: ExitGap) {
    let deps = &core.inner.deps;
    let venue = g.venue.0.clone();
    let r6 = |d: Decimal| d.round_dp(6).normalize();
    let Some(gap) = gap_pct(g.model_usdt, g.real_usdt) else {
        return;
    };
    let seen = seen_tax_pct(g.model_usdt, g.real_usdt, g.model_tax_pct).round_dp(4);
    let what = format!(
        "{} {}: реальный выход продажи {} USDT, модель {} USDT (ниже на {}%; налог в модели {}%, по замеру ~{}%)",
        g.deal_id,
        g.asset.ticker,
        r6(g.real_usdt),
        r6(g.model_usdt),
        gap.round_dp(4).normalize(),
        g.model_tax_pct.round_dp(4).normalize(),
        seen.normalize()
    );
    let s = &deps.settings;
    let max = match s.decimal("guard.exit_gap_pct", At::default()) {
        Ok(m) => m,
        Err(e) => {
            let text = format!("{what} — монета не закрыта для входа: {}", e.message);
            deps.log(Level::Info, "core.exit_gap", Some(&venue), &text, None)
                .await;
            return;
        }
    };
    if gap <= max {
        let text = format!("{what} — в пределах допуска guard.exit_gap_pct {max}%");
        deps.log(Level::Info, "core.exit_gap", Some(&venue), &text, None)
            .await;
        return;
    }
    if let Some(note) = super::blacklist::manual(s, &g.asset, &[]) {
        let text = format!(
            "{what} — монета уже в ручном чёрном списке ({}), отметка не менялась",
            note.unwrap_or_default()
        );
        deps.log(Level::Warn, "core.exit_gap", Some(&venue), &text, None)
            .await;
        deps.notify(CODE, Some(&venue), &text).await;
        return;
    }
    let note = format!(
        "{NOTE_HEAD}: монета закрыта для входа: реальный выход продажи ниже модели (налог в модели {}%, по замеру ~{}%) — {what}, допуск guard.exit_gap_pct {max}%. Снимется само, когда перепроверка налогов даст налог продажи, сходящийся с замером, или снимите отметку на «Монетах»",
        g.model_tax_pct.round_dp(4).normalize(),
        seen.normalize()
    );
    let scope = super::blacklist::coin_scope(&g.asset);
    let set = async {
        let seen_v = Value::String(seen.normalize().to_string());
        s.set(
            "coin.exit_tax_seen_pct",
            scope.clone(),
            Some(seen_v),
            AUTHOR,
        )
        .await?;
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
    let text = match set.await {
        Ok(_) => format!("{}: {note}", g.asset.ticker),
        Err(e) => format!(
            "{}: {note}; закрыть не удалось ({e}) — внесите вручную",
            g.asset.ticker
        ),
    };
    deps.log(Level::Warn, "core.exit_gap", Some(&venue), &text, None)
        .await;
    deps.notify(CODE, Some(&venue), &text).await;
}

/// Свежая проверка налогов монеты: авто-закрытие решения 248 снимается, если налог продажи сошёлся с замером
/// (matches). Человек уже снял отметку — налог по замеру стирается молча. Ошибок нет.
pub async fn on_check(core: &Core, c: &CoinCheck) {
    let CheckState::Known(t) = &c.taxes else {
        return;
    };
    let deps = &core.inner.deps;
    let s = &deps.settings;
    let scope = super::blacklist::coin_scope(&c.asset);
    let Some(seen) = s
        .get_exact("coin.exit_tax_seen_pct", &scope)
        .and_then(|v| v.as_str().and_then(|x| x.parse::<Decimal>().ok()))
    else {
        return;
    };
    let ours = s
        .get_exact("coin.blocked", &scope)
        .and_then(|v| v.as_bool())
        == Some(true)
        && s.get_exact("coin.blocked_note", &scope)
            .and_then(|v| v.as_str().map(|n| n.starts_with(NOTE_HEAD)))
            == Some(true);
    if !ours {
        let _ = s.set("coin.exit_tax_seen_pct", scope, None, AUTHOR).await;
        return;
    }
    let Ok(max) = s.decimal("guard.exit_gap_pct", At::default()) else {
        return;
    };
    if !matches(seen, t.sell_pct, max) {
        return;
    }
    let set = async {
        s.set("coin.blocked", scope.clone(), None, AUTHOR).await?;
        s.set("coin.blocked_note", scope.clone(), None, AUTHOR)
            .await?;
        s.set("coin.exit_tax_seen_pct", scope, None, AUTHOR).await
    };
    let head = format!(
        "{}: проверка налогов ({}) дала налог продажи {}% — сходится с замером ~{}% (допуск guard.exit_gap_pct {max}%)",
        c.asset.ticker,
        c.venue.0,
        t.sell_pct.round_dp(4).normalize(),
        seen.normalize()
    );
    let text = match set.await {
        Ok(_) => format!("{head}: монета снова открыта для входа (решение 248)"),
        Err(e) => format!("{head}; снять отметку не удалось ({e}) — снимите на «Монетах»"),
    };
    deps.log(Level::Info, "core.exit_gap", Some(&c.venue.0), &text, None)
        .await;
    deps.notify(CODE, Some(&c.venue.0), &text).await;
}
