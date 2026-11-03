//! Файл: core/window_view.rs
//! Что делает: данные экрана «Окна» (решение 125): страница списка окон с фильтрами (монета, со сделкой / без,
//! плюсовая / минусовая) — открытые с живыми счётчиками из памяти, размер — с диска; почему окна не пишутся; одно окно —
//! кадры из файла (с offset, не больше limit) разобранные адаптером своей площадки (стакан на момент кадра, сделки),
//! время площадки и время получения ботом, стакан всей глубины после каждого кадра изменений (window_book.rs: полный +
//! изменения с начала файла, верхние top уровней), наши сделки окна с хронологией; путь файла для «Скачать файл».
//! Кто вызывает: api/windows.rs.
//! От чего зависит: window.rs, window_cfg.rs, window_tap.rs (parse), storage/windows_db.rs, storage/episode_files.rs,
//! шлюз (Venue::decode_tap).
//! Настройки: window.* (только показ причины, если не заданы).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::core::types::{TapDecoded, TapView};
use crate::gateway::contract::Venue;
use crate::gateway::tap::TapStream;
use crate::storage::{episode_files, windows_db};

use super::engine::{Core, lk};
use super::window::WindowInfo;
use super::window_book::Book;
use super::window_cfg::Cfg;

impl Core {
    /// Что делает: страница списка окон, новые сверху. Принимает: фильтры, сколько, сколько пропустить.
    /// Возвращает: {windows, total, off, used_bytes}. Ошибки: текст сбоя базы.
    pub async fn windows(
        &self,
        f: windows_db::Filter,
        limit: u32,
        offset: u32,
    ) -> Result<Value, String> {
        let (rows, total) = windows_db::page(&self.inner.deps.db, f, limit, offset)
            .await
            .map_err(|e| e.to_string())?;
        let open = lk(&self.inner.windows).list();
        let files = &self.inner.window_files;
        let list: Vec<WindowInfo> = rows
            .iter()
            .filter_map(|b| serde_json::from_str::<WindowInfo>(b).ok())
            .map(|w| {
                let mut w = match open.iter().find(|o| o.id == w.id) {
                    // Открытое: направления и прибыль — из памяти, счётчики записи — из последней сводки задачи.
                    Some(o) => WindowInfo {
                        directions: o.directions.clone(),
                        max_profit: o.max_profit,
                        max_profit_unbounded: o.max_profit_unbounded,
                        ..w
                    },
                    None => w,
                };
                w.bytes = files.size(&w.id);
                w
            })
            .collect();
        Ok(json!({
            "windows": list,
            "total": total,
            "off": Cfg::read(&self.inner.deps.settings).err(),
            "used_bytes": files.used(),
        }))
    }

    /// Что делает: окно — сводка, служебные строки файла (открытие, включение записи, вход, закрытие), кадры с offset
    /// не больше limit (разобранные; у изменений — стакан после кадра, top уровней), сделки окна. Принимает: id, offset,
    /// limit, top.
    /// Возвращает: {info, lines, frames, frames_read, truncated, deals}; None — нет окна. Ошибки: база, файл.
    pub async fn window(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
        top: usize,
    ) -> Result<Option<Value>, String> {
        let Some(body) = windows_db::get(&self.inner.deps.db, id)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let info: WindowInfo = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let path = self.inner.window_files.path(id);
        let p = path.clone();
        // Чтение и распаковка — блокирующие, вне потоков async. Предел строк — служебные + кадры до offset + limit.
        let max = offset
            .saturating_add(limit)
            .saturating_add(1 + 2 * info.venues.len() + info.directions.len() + 1);
        let (rows, truncated) = tokio::task::spawn_blocking(move || episode_files::read(&p, max))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("файл окна {}: {e}", path.display()))?;
        let mut venues: HashMap<String, Arc<dyn Venue>> = HashMap::new();
        let (mut lines, mut frames, mut n) = (vec![], vec![], 0usize);
        // Стакан всей глубины по биржам (дополнение решения 125): изменения применяются с начала файла, в том числе до offset.
        let mut books: HashMap<String, Book> = HashMap::new();
        for r in rows
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        {
            let Some(f) = super::window_tap::parse(&r) else {
                lines.push(r);
                continue;
            };
            n += 1;
            let diff = matches!(f.stream, TapStream::Diff | TapStream::Snapshot);
            let shown = n > offset && frames.len() < limit;
            if !shown && !diff {
                continue;
            }
            if !venues.contains_key(&f.venue.0)
                && let Ok(v) = self.inner.deps.gw.get(&f.venue).await
            {
                venues.insert(f.venue.0.clone(), v);
            }
            let decoded = match venues.get(&f.venue.0) {
                Some(v) => v.decode_tap(f.stream, &f.data).map_err(|e| e.message),
                None => Err(format!(
                    "площадка {} не подключена — кадр не разобран",
                    f.venue.0
                )),
            };
            let book = decoded.as_ref().ok().and_then(|d| {
                books
                    .entry(f.venue.0.clone())
                    .or_default()
                    .apply(&d.view, top)
            });
            if !shown {
                continue;
            }
            // Изменения и снимок — без самих уровней (у полного их тысячи): сводка кадра и стакан после него.
            let decoded = match (diff, decoded) {
                (true, Ok(d)) => Ok(compact(&d)),
                (_, d) => d.map(|d| json!(d)),
            };
            frames.push(json!({
                "i": n - 1, "venue": f.venue.0, "stream": f.stream, "conn": f.conn, "recv_ns": f.recv_ns,
                "bytes": f.data.len(), "decoded": decoded, "book": book,
            }));
        }
        let deals: Vec<Value> = {
            let all = lk(&self.inner.deals);
            info.deals
                .iter()
                .filter_map(|d| all.get(&d.id))
                .map(|d| json!({ "id": d.id, "direction": d.direction, "step": d.step, "result_usd": d.result_usd, "timeline": d.timeline }))
                .collect()
        };
        Ok(Some(json!({
            "info": info, "lines": lines, "frames": frames, "frames_read": n, "truncated": truncated, "deals": deals,
        })))
    }

    /// Путь файла окна (скачивание). Принимает: id (только буквы, цифры, «-»). Возвращает: путь; None — неверный id.
    pub fn window_path(&self, id: &str) -> Option<std::path::PathBuf> {
        let ok = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        ok.then(|| self.inner.window_files.path(id))
    }
}

/// Сводка кадра изменений: время, полный ли, версии, сколько уровней изменилось. Ошибок нет.
fn compact(d: &TapDecoded) -> Value {
    match &d.view {
        TapView::Diff {
            bids,
            asks,
            full,
            from,
            to,
        } => json!({
            "exchange_ms": d.exchange_ms, "full": full, "from": from, "to": to, "changed": [bids.len(), asks.len()],
        }),
        v => json!({ "exchange_ms": d.exchange_ms, "view": v }),
    }
}
