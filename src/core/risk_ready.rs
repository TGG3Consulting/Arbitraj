//! Файл: core/risk_ready.rs
//! Что делает: заданы ли настройки, без которых торговля по направлению не включается (вынесено из risk.rs):
//! все обязательные money.* и risk.* (решение 15; необязательные — например risk.max_daily_loss_total_usd — не
//! требуются), risk.max_errors_in_row у биржи продажи (решение 118, Я-10), risk.max_daily_loss_usd у пары сделки
//! «площадка × тип направления» (решение 258: значение пары → площадки → вся система, day_pair.rs — тот же порядок, что
//! у проверки лимита).
//! Кто вызывает: risk.rs (can_open).
//! От чего зависит: settings (catalog, store), day_pair.rs (пара сделки и её лимит).
//! Решение 242: money.* с областью «площадка + тип направления» — заданы и значением пары (core/venue_dir.rs).
//! Решение 414: вместо risk.max_daily_loss_usd у пары может быть задан предел просадки от пика (risk.drawdown_pct /
//! risk.drawdown_max_usd, drawdown_cfg.rs) — тогда нужен снятый слепок баланса направления (drawdown_snap.rs).
//! Настройки: все обязательные money.* и risk.*, risk.max_errors_in_row, risk.max_daily_loss_usd или
//! risk.drawdown_pct / risk.drawdown_max_usd.

use crate::core::types::Direction;
use crate::settings::catalog;
use crate::settings::schema::ScopeKind;

use super::engine::Core;

/// Что делает: проверка настроек направления. Принимает: ядро, направление.
/// Возвращает: Ok или Err — «торговля не включается: не задана настройка …».
pub(super) fn settings_ready(core: &Core, d: &Direction) -> Result<(), String> {
    let s = &core.inner.deps.settings;
    // Решение 242: у настроек с областью «площадка + тип направления» значение пары тоже считается заданным (как его
    // читает расчёт — venue_dir.rs); не задано для пары — как раньше.
    let vd = super::venue_dir::of(core, d);
    // Решение 258: risk.max_daily_loss_usd — ниже, у пары сделки (площадка второго шага — биржа пары, а не площадка
    // покупки, как у остальных).
    for def in catalog::all().filter(|d| {
        !d.optional
            && d.name != super::day_pair::SETTING
            && (d.name.starts_with("money.") || d.name.starts_with("risk."))
    }) {
        let pair = vd.filter(|_| def.scopes.contains(&ScopeKind::VenueDir));
        if s.resolve_dir(def.name, pair, Some(&d.buy_on.0), None)
            .is_none()
        {
            return Err(format!(
                "торговля не включается: не задана настройка {}",
                def.name
            ));
        }
    }
    // Решение 118 (Я-10): у биржи продажи — своя пауза после ошибок подряд; без неё B на сбое не встанет на паузу.
    if s.resolve("risk.max_errors_in_row", Some(&d.sell_on.0), None)
        .is_none()
    {
        return Err(format!(
            "торговля не включается: не задана настройка risk.max_errors_in_row [venue:{}]",
            d.sell_on.0
        ));
    }
    // Решение 258: у пары сделки — свой лимит дня (значение пары, площадки или «вся система»).
    // Решение 414: или предел просадки от пика (risk.drawdown_pct / risk.drawdown_max_usd) — тогда старый не нужен.
    let pair = super::day_pair::of_direction(core, d);
    if super::drawdown_cfg::active(core, &pair) {
        return snap_ready(core, &pair);
    }
    if !super::day_pair::is_set(core, &pair) {
        return Err(format!(
            "торговля не включается: не задана настройка {} [venue_dir:{pair}] — ни у направления {}, ни у площадки, ни у всей системы (и не задан предел просадки {} / {}, решение 414)",
            super::day_pair::SETTING,
            super::day_pair::label(&pair),
            super::drawdown_cfg::PCT,
            super::drawdown_cfg::USD
        ));
    }
    Ok(())
}

/// Что делает: решение 414 — у направления по просадке от пика должен быть слепок баланса (хоть раз снятый): без
/// него лимит не работает, и торговля направлением не включается — как при не заданном лимите (решение 258).
/// Принимает: ядро, id пары. Возвращает: Ok или Err с причиной (почему слепок ещё не снят). Ошибок нет.
fn snap_ready(core: &Core, pair: &str) -> Result<(), String> {
    let r = super::engine::lk(&core.inner.risk);
    if r.day.draw.contains_key(pair) {
        return Ok(());
    }
    let why = r
        .day
        .draw_want
        .get(pair)
        .and_then(|w| w.note.clone())
        .map_or_else(
            || "снимается — до 2 минут, пока идут сделки направления".to_string(),
            |n| format!("не снят: {n}"),
        );
    Err(format!(
        "торговля не включается: у направления {} ещё нет слепка баланса для лимита просадки (решение 414) — {why}",
        super::day_pair::label(pair)
    ))
}
