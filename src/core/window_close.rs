//! Файл: core/window_close.rs
//! Что делает: окна (решение 125), вторая половина задачи окон: сделки монеты в окне (из учёта ядра), закрытие окна
//! (выключить запись на биржах, дождаться последнего кадра, строка закрытия, сводка, журнал, Telegram window.closed),
//! сводка в SQLite и живой поток.
//! Кто вызывает: core/window_worker.rs, core/window_cfg.rs (save_row).
//! От чего зависит: window.rs, window_tap.rs, storage/windows_db.rs, core/types/window.rs (outcome).
//! Настройки: нет (window.* читает window_worker.rs).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use chrono::Utc;
use serde_json::json;

use crate::core::types::window::outcome;
use crate::storage::journal::Level;
use crate::storage::windows_db;

use super::coin_key::CoinKey;
use super::engine::{Core, lk};
use super::view::CoreEvent;
use super::window::{WinDeal, WindowInfo};
use super::window_cfg::warn;
use super::window_info::{mark, pairs};
use super::window_tap::{self, Rec};

/// Открытое окно в задаче: ключ монеты, запись, что включено на биржах, сделки.
pub struct Run {
    pub key: CoinKey,
    pub rec: Rec,
    pub taps: HashMap<String, Result<crate::gateway::tap::TapInfo, String>>,
    pub deals: Vec<WinDeal>,
}

/// Сделки монеты, начатые не раньше окна, — из учёта ядра. Ошибок нет.
pub fn deals(core: &Core, st: &mut HashMap<String, Run>) {
    let wins = lk(&core.inner.windows).list();
    let all = lk(&core.inner.deals);
    for w in wins {
        let Some(r) = st.get_mut(&w.id) else { continue };
        r.deals = all
            .values()
            .filter(|d| CoinKey::of(&d.direction.asset) == r.key && d.started_at >= w.started_at)
            .map(|d| WinDeal {
                id: d.id.clone(),
                open: d.is_open(),
                step: format!("{:?}", d.step),
                result_usd: d.result_usd,
            })
            .collect();
        r.deals.sort_by(|a, b| a.id.cmp(&b.id));
    }
}

/// Закрывает окно: запись на биржах выключена, последний кадр дописан, строка закрытия, сводка. Ошибок нет.
pub async fn close(core: &Core, st: &mut HashMap<String, Run>, k: &CoinKey, why: &str) {
    let Some(mut info) = lk(&core.inner.windows).close(k, why, Utc::now()) else {
        return;
    };
    if let Some(r) = st.remove(&info.id) {
        let mut all_off = true;
        for w in &info.venues {
            if matches!(r.taps.get(&w.venue.0), Some(Ok(_))) {
                all_off &= window_tap::untap(core, &w.venue, &w.asset).await;
            }
        }
        let Run {
            rec, taps, deals, ..
        } = r;
        let Rec {
            sink,
            lost,
            stats,
            task,
        } = rec;
        drop(sink);
        if all_off {
            // Все приёмники закрыты — задача записи дописывает очередь и заканчивается.
            let _ = task.await;
        } else {
            // Запись на бирже не выключилась — приёмник у адаптера жив, задача не кончится сама: снять её.
            task.abort();
        }
        fill(
            &mut info,
            &taps,
            &deals,
            &lk(&stats),
            lost.load(Ordering::Relaxed),
        );
    }
    let files = &core.inner.window_files;
    files.line(
        &info.id,
        json!({ "type": "close", "info": info }).to_string(),
    );
    files.close(&info.id);
    info.bytes = files.size(&info.id);
    save_row(core, &info).await;
    let secs = info
        .ended_at
        .map(|e| (e - info.started_at).num_milliseconds() as f64 / 1000.0)
        .unwrap_or(0.0);
    let deal = match (info.deals.is_empty(), info.outcome.as_deref()) {
        (true, _) => "сделки не было".to_string(),
        (false, o) => format!(
            "БЫЛА СДЕЛКА ({})",
            o.map_or("итога нет", |o| match o {
                "plus" => "плюсовая",
                "minus" => "минусовая",
                _ => "ноль",
            })
        ),
    };
    let text = format!(
        "{}Окно закрыто: {} — {secs:.1} с, кадров {}, потеряно {}, {deal}; {why}",
        mark(&info),
        pairs(&info),
        info.frames,
        info.lost
    );
    log(core, text.clone(), Some(&info)).await;
    core.inner.deps.notify("window.closed", None, &text).await;
}

/// Счётчики, итог по биржам и сделки — в сводку. Ошибок нет.
fn fill(
    info: &mut WindowInfo,
    taps: &HashMap<String, Result<crate::gateway::tap::TapInfo, String>>,
    deals: &[WinDeal],
    s: &window_tap::Stats,
    lost: u64,
) {
    for w in &mut info.venues {
        w.tap = taps.get(&w.venue.0).cloned();
    }
    info.frames = s.frames;
    info.lost = lost;
    info.stopped = s.stopped.clone();
    info.deals = deals.to_vec();
    let sum = (!deals.is_empty() && deals.iter().all(|d| d.result_usd.is_some()))
        .then(|| deals.iter().filter_map(|d| d.result_usd).sum());
    info.outcome = outcome(sum).map(str::to_string);
}

/// Сводка открытого окна (память ядра + счётчики задачи) — в SQLite и живой поток. Возвращает: сводку; None — окна
/// уже нет в памяти (закрыто между сообщениями). Ошибок нет.
pub async fn save(core: &Core, st: &HashMap<String, Run>, id: &str) -> Option<WindowInfo> {
    let r = st.get(id)?;
    let mut info = lk(&core.inner.windows).get(&r.key).filter(|w| w.id == id)?;
    fill(
        &mut info,
        &r.taps,
        &r.deals,
        &lk(&r.rec.stats),
        r.rec.lost.load(Ordering::Relaxed),
    );
    info.bytes = core.inner.window_files.size(id);
    save_row(core, &info).await;
    Some(info)
}

/// Сводку — в SQLite и живой поток. Ошибок нет: сбой базы — в журнал.
pub async fn save_row(core: &Core, info: &WindowInfo) {
    core.inner.live.send(CoreEvent::Window(json!(info)));
    let Ok(body) = serde_json::to_string(info) else {
        return;
    };
    let row = windows_db::Row {
        id: info.id.clone(),
        started_at: info.started_at.to_rfc3339(),
        ticker: info.asset.ticker.clone(),
        // Решение 211: адрес монеты — для фильтра окон по адресу.
        address: info.asset.bsc_contract.map(|a| format!("{a:#x}")),
        deal: !info.deals.is_empty(),
        outcome: info.outcome.clone(),
        body,
    };
    if let Err(e) = windows_db::save(&core.inner.deps.db, row).await {
        warn(core, format!("окно {}: {e}", info.id)).await;
    }
}

/// Журнал. Ошибок нет.
pub async fn log(core: &Core, text: String, info: Option<&WindowInfo>) {
    core.inner
        .deps
        .log(
            Level::Info,
            "core.window",
            None,
            text,
            info.map(|i| json!(i)),
        )
        .await;
}
