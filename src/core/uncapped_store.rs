//! Файл: core/uncapped_store.rs
//! Что делает: запись итогов расчёта «без потолка» (решение 141) для анализа. Сделка — одна строка на расчёт (вид deal,
//! с id сделки). Кандидат — ЭПИЗОД, а не каждый тик: пока расчёты направления идут с промежутком не длиннее
//! uncapped.episode_gap_ms, это одна строка — в ней пиковая по упущенному запись (и время пика), первый и последний
//! расчёт, сколько расчётов и в скольких бюджет срезал оптимум; промежуток длиннее — новая строка. Выбор: эпизод
//! хранит главное для анализа (сколько максимум теряли и как долго) без тысяч одинаковых строк; uncapped.episode_gap_ms =
//! 0 — каждый расчёт своей строкой (прореживание — интервалом uncapped.min_interval_ms очереди).
//! Срок хранения — uncapped.keep_days: строки, начатые раньше, удаляются при каждой новой строке. Строка «Кандидатов»
//! направления получает поле uncapped («без потолка: вложить X → прибыль Y; плюс от Z»). Сбой — счётчик и причина
//! (экран), в журнал — на 1-й, 2-й, 4-й… сбой.
//! Решение 197 (Тигран, 06.11.2026): окно — измеритель упущенной прибыли, поэтому кандидат пишется в базу, только если
//! потолок реально помешал — упущено > 0 (без потолка проходит пороги прибыли, а на нашем бюджете — нет или меньше);
//! упущено 0 — только счётчик no_missed и поле строки «Кандидатов». Эпизод продолжают только такие расчёты: его первый и
//! последний расчёт — окно, пока упущенное было (промежутки не длиннее uncapped.episode_gap_ms). Сделка пишется всегда
//! (и с упущено 0 — «потолок не мешал» тоже знание).
//! Кто вызывает: core/uncapped_queue.rs.
//! От чего зависит: uncapped_state.rs, uncapped_types.rs, storage/uncapped_db.rs, view.rs, settings.
//! Настройки: uncapped.episode_gap_ms, uncapped.keep_days.

use std::time::{Duration, Instant};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::core::types::Direction;
use crate::settings::read::At;
use crate::storage::journal::Level;
use crate::storage::uncapped_db::{self, Row};

use super::engine::{Core, lk};
use super::uncapped_state::{Key, OpenEp};
use super::uncapped_types::{Episode, Record, budget_bound};
use super::view::CoreEvent;

/// Время для столбцов базы: RFC 3339 с миллисекундами и Z (строки сравниваются по порядку). Ошибок нет.
pub fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Сбой расчёта или настройки: счётчик, последняя причина (экран); в журнал — на 1-й, 2-й, 4-й, 8-й… сбой (причины с
/// числами разные на каждый расчёт — журнал не засоряется, как потери задачи эпизодов). Ошибок нет.
pub fn failed(core: &Core, e: String) {
    let n = {
        let mut s = lk(&core.inner.uncapped.stats);
        s.failed += 1;
        s.last_error = Some(e.clone());
        s.failed
    };
    if n.is_power_of_two() {
        let core = core.clone();
        tokio::spawn(async move {
            let text = format!("Расчёт без потолка: сбой №{n}: {e}");
            core.inner
                .deps
                .log(Level::Warn, "core.uncapped", None, text, None)
                .await;
        });
    }
}

/// Решение 217: сбой расчёта по сделке — как failed (счётчик, причина экрана) и причина по id сделки для сообщения о
/// закрытии (deal_ceiling.rs: «не посчитано: …»); причины старше uncapped.keep_days убираются. Ошибок нет.
pub fn deal_failed(core: &Core, id: &str, e: String) {
    let now = Utc::now();
    let keep = core
        .inner
        .deps
        .settings
        .integer("uncapped.keep_days", At::default())
        .ok();
    {
        let mut m = lk(&core.inner.uncapped.deal_errors);
        let from = keep
            .and_then(|k| i64::try_from(k).ok())
            .and_then(chrono::Duration::try_days)
            .and_then(|d| now.checked_sub_signed(d));
        if let Some(from) = from {
            m.retain(|_, (t, _)| *t >= from);
        }
        m.insert(id.to_string(), (now, e.clone()));
    }
    failed(core, e);
}

/// Сохраняет итог (см. шапку). Принимает: ядро, ключ работы, запись. Ошибок нет — сбой в failed.
pub async fn save(core: &Core, key: &Key, mut rec: Record) {
    let int = |n: &str| core.inner.deps.settings.integer(n, At::default());
    let (gap, keep) = match (int("uncapped.episode_gap_ms"), int("uncapped.keep_days")) {
        (Ok(g), Ok(k)) => (Duration::from_millis(g), k),
        (Err(e), _) | (_, Err(e)) => return failed(core, e.message),
    };
    {
        let mut s = lk(&core.inner.uncapped.stats);
        s.done += 1;
        s.total_ms += rec.ms;
        s.max_ms = s.max_ms.max(rec.ms);
    }
    let bound = budget_bound(&rec.limited_by);
    rec.ep.capped_count = u64::from(bound);
    let (kind, deal_id, dir) = match key {
        Key::Deal(id) => ("deal", Some(id.clone()), None),
        Key::Cand(d) => {
            publish(core, d, &rec);
            if rec.missed <= rust_decimal::Decimal::ZERO {
                lk(&core.inner.uncapped.stats).no_missed += 1;
                return;
            }
            ("candidate", None, Some(d.clone()))
        }
    };
    let body = serde_json::to_value(&rec).unwrap_or_default();
    let db = &core.inner.deps.db;
    // Эпизод кандидата продолжается — обновить строку.
    if let Some(d) = &dir
        && let Some((id, body)) = merge(core, d, &rec, body.clone(), bound, gap)
    {
        if let Err(e) = uncapped_db::update(db, id, ts(rec.at), body.to_string()).await {
            failed(core, format!("запись в базу: {e}"));
        }
        return;
    }
    let row = Row {
        kind,
        deal_id,
        ticker: rec.direction.asset.ticker.clone(),
        // Решение 211: адрес монеты (столбец address).
        address: rec.direction.asset.bsc_contract.map(|a| format!("{a:#x}")),
        started_at: ts(rec.at),
        body: body.to_string(),
    };
    match uncapped_db::insert(db, row).await {
        Ok(id) => {
            if let Some(d) = dir {
                let ep = OpenEp {
                    id,
                    peak: rec.missed,
                    last: Instant::now(),
                    body,
                };
                lk(&core.inner.uncapped.episodes).insert(d, ep);
            }
        }
        Err(e) => return failed(core, format!("запись в базу: {e}")),
    }
    // Срок больше, чем умеет chrono, — уборки нет (такие строки не устареют).
    let Some(before) = i64::try_from(keep)
        .ok()
        .and_then(chrono::Duration::try_days)
        .and_then(|k| rec.at.checked_sub_signed(k))
    else {
        return;
    };
    if let Err(e) = uncapped_db::prune(db, ts(before)).await {
        failed(core, format!("уборка по uncapped.keep_days: {e}"));
    }
}

/// Продолжение открытого эпизода (gap > 0 и промежуток не длиннее): пик — запись с большим упущенным, счётчики.
/// Возвращает: (id строки, новый JSON); None — эпизода нет или он кончился. Ошибок нет.
fn merge(
    core: &Core,
    d: &Direction,
    rec: &Record,
    body: Value,
    bound: bool,
    gap: Duration,
) -> Option<(i64, Value)> {
    if gap.is_zero() {
        return None;
    }
    let mut eps = lk(&core.inner.uncapped.episodes);
    let op = eps.get_mut(d).filter(|op| op.last.elapsed() <= gap)?;
    let mut ep: Episode = serde_json::from_value(op.body["ep"].clone()).ok()?;
    ep.count += 1;
    ep.capped_count += u64::from(bound);
    ep.last_at = rec.at;
    if rec.missed > op.peak {
        op.peak = rec.missed;
        ep.peak_at = rec.at;
        op.body = body;
    }
    op.body["ep"] = serde_json::to_value(&ep).unwrap_or_default();
    op.last = Instant::now();
    Some((op.id, op.body.clone()))
}

/// Поле uncapped строки «Кандидатов» направления и живой поток. Ошибок нет.
fn publish(core: &Core, d: &Direction, rec: &Record) {
    let row = {
        let mut rows = lk(&core.inner.candidates);
        let Some(row) = rows.get_mut(d) else { return };
        row["uncapped"] = json!({
            "at": rec.at,
            "budget": rec.budget,
            "opt": rec.opt,
            "opt_passes": rec.opt_passes,
            "min_plus": rec.min_plus,
            "at_budget": rec.at_budget,
            "missed": rec.missed,
            "limited_by": rec.limited_by,
            "limit": rec.limit,
            "ms": rec.ms,
        });
        row.clone()
    };
    core.inner.live.send(CoreEvent::Candidate(row));
}
