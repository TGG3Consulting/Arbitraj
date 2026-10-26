//! Файл: core/episode_run.rs
//! Что делает: ведёт эпизоды арбитражной ситуации (решение 74) — открытие по прошедшему кандидату (в том числе сухому),
//! записи пересчётов (episode_snap.rs) в файл эпизода не чаще episode.min_interval_ms, закрытие, когда направление
//! перестало проходить (отказ расчёта или сигнал вышел из «кандидата»), с причиной. Журнал и Telegram — только
//! открытие (episode.opened) и закрытие с итогом (episode.closed: длительность, наибольшая прибыль, причина) —
//! пересчёты идут только в файл. Сводка — в SQLite (storage/episodes_db.rs), живой поток — событие episode.
//! Файлы не помещаются в episode.max_mb даже после удаления старых — запись открытого эпизода останавливается (причина в
//! сводке), сам эпизод живёт до закрытия.
//! Кто вызывает: core/candidates.rs (accepted, failed), core/evaluate.rs (failed — сигнал вышел из «кандидата»).
//! От чего зависит: episode.rs, episode_cfg.rs, episode_snap.rs, card.rs, storage/episode_files.rs, storage/episodes_db.rs.
//! Настройки: episode.* (episode_cfg.rs).

use chrono::Utc;
use serde_json::json;

use crate::core::types::Direction;
use crate::storage::episodes_db;
use crate::storage::journal::Level;

use super::card::Card;
use super::costs::Calc;
use super::engine::{Core, lk};
use super::episode::{EpisodeInfo, Step};
use super::episode_cfg::{Cfg, cleanup, current};
use super::view::CoreEvent;

/// Что делает: кандидат прошёл — открыть эпизод или записать пересчёт.
/// Принимает: ядро, расчёт, карточку, сухой ли режим. Возвращает: None — эпизоды не ведутся (не заданы episode.*);
/// Some(true) — эпизод открыт сейчас; Some(false) — уже был открыт. Ошибок нет.
pub async fn accepted(core: &Core, calc: &Calc, card: &Card, dry: bool) -> Option<bool> {
    let cfg = current(core).await?;
    let d = &calc.direction;
    let now = Utc::now();
    let unbounded = card.max.as_ref().ok().map(|m| m.opt_profit);
    let step = lk(&core.inner.episodes).accept(
        d,
        new_id(d, now),
        dry,
        calc.profit_usd,
        unbounded,
        now,
        cfg.min_interval_ms,
    );
    let files = &core.inner.episode_files;
    match step {
        Step::Skip => Some(false),
        Step::Record(info) => {
            if room(core, &cfg, d, &info).await {
                let line = super::episode_snap::record(core, calc, card, cfg.book_levels);
                files.line(&info.id, line.to_string());
            }
            save(core, &info).await;
            Some(false)
        }
        Step::Opened(info) => {
            files.open(&info.id, cfg.zstd_level);
            files.line(
                &info.id,
                json!({ "type": "open", "info": info, "card": card }).to_string(),
            );
            files.line(
                &info.id,
                super::episode_snap::record(core, calc, card, cfg.book_levels).to_string(),
            );
            save(core, &info).await;
            let text = format!(
                "{}Эпизод открыт: {}; прибыль {} USDT на бюджете {}; id {}",
                if dry { "[СУХОЙ] " } else { "" },
                pair(d),
                calc.profit_usd.round_dp(4),
                card.budget
                    .map(|b| b.normalize().to_string())
                    .unwrap_or_default(),
                info.id
            );
            tell(core, "episode.opened", d, &text, &info).await;
            Some(true)
        }
    }
}

/// Что делает: направление перестало проходить — закрыть открытый эпизод с причиной.
/// Принимает: ядро, направление, причину. Ошибок нет (эпизода нет — ничего).
pub async fn failed(core: &Core, d: &Direction, reason: &str) {
    let Some(info) = lk(&core.inner.episodes).close(d, reason, Utc::now()) else {
        return;
    };
    let files = &core.inner.episode_files;
    files.line(
        &info.id,
        json!({ "type": "close", "info": info }).to_string(),
    );
    files.close(&info.id);
    save(core, &info).await;
    let secs = info
        .ended_at
        .map(|e| (e - info.started_at).num_milliseconds() as f64 / 1000.0)
        .unwrap_or_default();
    let text = format!(
        "Эпизод закрыт: {}; длительность {secs:.1} с; макс. прибыль {} USDT{}; записей {}; причина: {reason}",
        pair(d),
        info.max_profit.round_dp(4),
        info.max_profit_unbounded
            .map(|v| format!(" (без бюджета {})", v.round_dp(4)))
            .unwrap_or_default(),
        info.records
    );
    tell(core, "episode.closed", d, &text, &info).await;
    if let Ok(cfg) = Cfg::read(&core.inner.deps.settings) {
        cleanup(core, &cfg).await;
    }
}

/// Открыт ли эпизод направления. Ошибок нет.
pub fn is_open(core: &Core, d: &Direction) -> bool {
    lk(&core.inner.episodes).is_open(d)
}

/// Есть ли место для записи; нет — удаление старых, не помогло — запись эпизода остановлена (один раз). Ошибок нет.
async fn room(core: &Core, cfg: &Cfg, d: &Direction, info: &EpisodeInfo) -> bool {
    if info.stopped.is_some() {
        return false;
    }
    let files = &core.inner.episode_files;
    if files.used() <= cfg.max_bytes {
        return true;
    }
    cleanup(core, cfg).await;
    if files.used() <= cfg.max_bytes {
        return true;
    }
    let why = format!(
        "лимит места episode.max_mb: занято {} МБ — запись остановлена",
        files.used() / (1024 * 1024)
    );
    lk(&core.inner.episodes).stop(d, &why);
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.episode",
            Some(&d.buy_on.0),
            format!("{}: {why}", info.id),
            None,
        )
        .await;
    false
}

/// Сводку — в SQLite и живой поток. Ошибок нет: сбой базы — в журнал.
async fn save(core: &Core, info: &EpisodeInfo) {
    core.inner.live.send(CoreEvent::Episode(json!(info)));
    let Ok(body) = serde_json::to_string(info) else {
        return;
    };
    let started = info.started_at.to_rfc3339();
    if let Err(e) = episodes_db::save(&core.inner.deps.db, &info.id, started, body).await {
        core.inner
            .deps
            .log(
                Level::Warn,
                "core.episode",
                None,
                format!("эпизод {}: {e}", info.id),
                None,
            )
            .await;
    }
}

/// Журнал и Telegram. Ошибок нет.
async fn tell(core: &Core, code: &str, d: &Direction, text: &str, info: &EpisodeInfo) {
    let deps = &core.inner.deps;
    deps.log(
        Level::Info,
        "core.episode",
        Some(&d.buy_on.0),
        text,
        Some(json!(info)),
    )
    .await;
    deps.notify(code, Some(&d.buy_on.0), text).await;
}

/// «CAKE bingx → bsc_dex». Ошибок нет.
fn pair(d: &Direction) -> String {
    format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0)
}

/// id эпизода: время до мс, тикер, площадки — только буквы, цифры и «-» (годится как имя файла). Ошибок нет.
fn new_id(d: &Direction, now: chrono::DateTime<Utc>) -> String {
    let clean = |s: &str| -> String { s.chars().filter(char::is_ascii_alphanumeric).collect() };
    format!(
        "{}-{}-{}-{}",
        now.format("%Y%m%d%H%M%S%3f"),
        clean(&d.asset.ticker),
        clean(&d.buy_on.0),
        clean(&d.sell_on.0)
    )
}
