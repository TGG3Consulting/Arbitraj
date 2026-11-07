//! Файл: core/uncapped_recompute_all.rs
//! Что делает: решение 218 — пересчёт старых записей «без потолка» по всем сделкам: чтение базы бота (сделки из учёта
//! core_state, записи сделок таблицы uncapped, сводки окон) и проход по сделкам — у кого последняя запись старая
//! (live_after_buy), тот пересчитывается (uncapped_recompute.rs one), у остальных — строка «не пересчитывается».
//! Кто вызывает: src/bin/uncapped_recompute.rs, тесты (tests/uncapped_recompute.rs).
//! От чего зависит: uncapped_recompute.rs, persist.rs, ledger.rs (KIND), storage (uncapped_db, windows_db).
//! Настройки: нет (настройки модели — uncapped_recompute_ctx.rs).

use std::collections::HashMap;

use crate::storage::db::Db;
use crate::storage::uncapped_db::{self, DealRow};
use crate::storage::windows_db;

use super::deal::Deal;
use super::uncapped_recompute::{Env, Item, one, one_skip, select};
use super::window::WindowInfo;

/// Всё из базы бота, что нужно пересчёту.
pub struct Base {
    pub deals: HashMap<String, Deal>,
    pub rows: Vec<DealRow>,
    pub windows: Vec<WindowInfo>,
}

/// Что делает: читает сделки (core_state), записи сделок (uncapped) и сводки окон (windows). Принимает: базу.
/// Возвращает: Base. Ошибки: текст сбоя базы.
pub async fn load(db: &Db) -> Result<Base, String> {
    let deals = super::persist::load::<Deal>(db, super::ledger::KIND)
        .await
        .map_err(|e| format!("сделки: {e}"))?;
    let rows = uncapped_db::deal_rows(db)
        .await
        .map_err(|e| format!("записи «без потолка»: {e}"))?;
    let windows = windows_db::oldest_first(db)
        .await
        .map_err(|e| format!("окна: {e}"))?
        .iter()
        .filter_map(|b| serde_json::from_str::<WindowInfo>(b).ok())
        .collect();
    Ok(Base {
        deals: deals.into_iter().map(|d| (d.id.clone(), d)).collect(),
        rows,
        windows,
    })
}

/// Что делает: пересчёт всех сделок, у которых последняя запись старая (only — только эти id; пусто — все); у уже
/// пересчитанных — строка «уже пересчитано». Принимает: окружение, базу, фильтр. Возвращает: строки по сделкам.
/// Ошибок нет.
pub async fn all(env: &Env<'_>, base: &Base, only: &[String]) -> Vec<Item> {
    let mut out = vec![];
    for (row, old) in select(base.rows.clone()) {
        if !only.is_empty() && !only.contains(&row.deal_id) {
            continue;
        }
        let deal = base.deals.get(&row.deal_id);
        if old {
            out.push(one(env, &row, deal, &base.windows).await);
            continue;
        }
        let basis = serde_json::from_str::<serde_json::Value>(&row.body)
            .ok()
            .and_then(|v| v["basis"].as_str().map(String::from))
            .unwrap_or_default();
        let mut it = one_skip(&row, deal);
        it.line.reason = Some(format!(
            "последняя запись не старая (basis {basis}) — не пересчитывается"
        ));
        out.push(it);
    }
    out
}
