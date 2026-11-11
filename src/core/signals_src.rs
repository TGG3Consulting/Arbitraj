//! Файл: core/signals_src.rs
//! Что делает: решение 255 — исходные данные отчёта «Сигналы и сделки» (report/sig_*.rs), собранные из того, что бот
//! хранит: сводки окон (таблица windows, storage/windows_db.rs; у открытых — направления и счётчики из памяти, как экран
//! «Окна», window_view.rs), сводки эпизодов (таблица episodes, storage/episodes_db.rs), сделки учёта ядра (core_state
//! kind = deal, Core::deals), вид площадок (ЦЕКС / ДЕКС — inner.kinds из шлюза), адаптеры площадок окон (разбор сырых
//! кадров — Venue::decode_tap; ядро форматов площадок не знает), пути файлов окон и эпизодов, причины, почему окна или
//! эпизоды не пишутся (window_cfg / episode_cfg). Только чтение: одна выборка из базы на таблицу, копии из памяти под
//! коротким замком; файлы читает и книгу собирает вызывающий — в spawn_blocking, торговля не ждёт.
//! Кто вызывает: report/mod.rs (signals_files).
//! От чего зависит: storage/windows_db.rs, storage/episodes_db.rs, window.rs, episode.rs, ledger.rs (deals), шлюз.
//! Настройки: window.*, episode.* (только причина «не пишутся», если не заданы).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::core::deal::Deal;
use crate::core::types::{TapDecoded, VenueId};
use crate::gateway::contract::{Venue, VenueKind};
use crate::gateway::tap::{TapData, TapStream};
use crate::storage::{episodes_db, windows_db};

use super::engine::{Core, lk, rd};
use super::episode::EpisodeInfo;
use super::window::WindowInfo;

/// Разбор кадра окна: площадка, поток, содержимое → кадр в формате ядра или текст ошибки.
pub type Decode =
    Arc<dyn Fn(&VenueId, TapStream, &TapData) -> Result<TapDecoded, String> + Send + Sync>;

/// Исходные данные отчёта (без файлов — их читает отчёт).
pub struct SignalsSrc {
    /// Все окна из базы (у открытых — данные из памяти), старые сверху.
    pub windows: Vec<WindowInfo>,
    /// Все эпизоды из базы (у открытых — из памяти), старые сверху.
    pub episodes: Vec<EpisodeInfo>,
    /// Все сделки учёта ядра.
    pub deals: Vec<Deal>,
    /// Вид площадок (id → ЦЕКС / ДЕКС); площадки нет — она не подключена сейчас.
    pub kinds: HashMap<String, VenueKind>,
    /// Разбор сырого кадра окна адаптером его площадки (Venue::decode_tap подключённых сейчас площадок; не подключена —
    /// ошибка с причиной). Функция, а не адаптеры, — как core/window_at.rs Decode: тесты передают разбор записанных кадров.
    pub decode: Decode,
    /// Папки файлов: окна (<data-dir>/windows), эпизоды (<data-dir>/episodes) — файл <id>.jsonl.zst.
    pub window_dir: PathBuf,
    pub episode_dir: PathBuf,
    /// Почему окна / эпизоды сейчас не пишутся (не заданы настройки); None — пишутся.
    pub windows_off: Option<String>,
    pub episodes_off: Option<String>,
    /// Состояние пулов в точках DEX (window.dex_pool_state): Some(причина) — не пишется сейчас.
    pub pool_state_off: Option<String>,
    pub now: DateTime<Utc>,
}

impl Core {
    /// Что делает: собирает исходные данные отчёта «Сигналы и сделки» (см. шапку). Принимает: время выгрузки.
    /// Возвращает: SignalsSrc. Ошибки: текст — база не прочитана.
    pub async fn signals_src(&self, now: DateTime<Utc>) -> Result<SignalsSrc, String> {
        let db = &self.inner.deps.db;
        let wrows = windows_db::oldest_first(db)
            .await
            .map_err(|e| format!("окна: {e}"))?;
        let erows = episodes_db::oldest_first(db)
            .await
            .map_err(|e| format!("эпизоды: {e}"))?;
        let open_w = lk(&self.inner.windows).list();
        let open_e = lk(&self.inner.episodes).list();
        let windows: Vec<WindowInfo> = wrows
            .iter()
            .filter_map(|b| serde_json::from_str::<WindowInfo>(b).ok())
            .map(|w| match open_w.iter().find(|o| o.id == w.id) {
                // Открытое: направления, площадки и прибыль — из памяти (сводка в базе — с последнего сохранения).
                Some(o) => WindowInfo {
                    directions: o.directions.clone(),
                    venues: o.venues.clone(),
                    max_profit: o.max_profit,
                    dex_points: o.dex_points.max(w.dex_points),
                    ..w
                },
                None => w,
            })
            .collect();
        let episodes: Vec<EpisodeInfo> = erows
            .iter()
            .filter_map(|b| serde_json::from_str::<EpisodeInfo>(b).ok())
            .map(|e| open_e.iter().find(|o| o.id == e.id).cloned().unwrap_or(e))
            .collect();
        let kinds: HashMap<String, VenueKind> = rd(&self.inner.kinds)
            .iter()
            .map(|(v, k)| (v.0.clone(), *k))
            .collect();
        let mut venues: HashMap<String, Arc<dyn Venue>> = HashMap::new();
        for v in windows.iter().flat_map(|w| &w.venues) {
            if venues.contains_key(&v.venue.0) {
                continue;
            }
            if let Ok(a) = self.inner.deps.gw.get(&v.venue).await {
                venues.insert(v.venue.0.clone(), a);
            }
        }
        let s = &self.inner.deps.settings;
        let pool_state_off = match s.flag(
            "window.dex_pool_state",
            crate::settings::read::At::default(),
        ) {
            Ok(true) => None,
            Ok(false) => Some("window.dex_pool_state = нет".into()),
            Err(e) => Some(e.message),
        };
        Ok(SignalsSrc {
            windows,
            episodes,
            deals: self.deals(),
            kinds,
            decode: decoder(venues),
            window_dir: dir_of(&self.inner.window_files.path("x")),
            episode_dir: dir_of(&self.inner.episode_files.path("x")),
            windows_off: super::window_cfg::Cfg::read(s).err(),
            episodes_off: super::episode_cfg::Cfg::read(s).err(),
            pool_state_off,
            now,
        })
    }
}

/// Разбор кадров адаптерами площадок окон. Принимает: адаптеры по id. Ошибок нет.
fn decoder(venues: HashMap<String, Arc<dyn Venue>>) -> Decode {
    Arc::new(move |v, s, d| match venues.get(&v.0) {
        Some(a) => a.decode_tap(s, d).map_err(|e| e.message),
        None => Err("площадка не подключена сейчас — кадр не разобран".into()),
    })
}

/// Папка файла (путь файла «x» → его папка). Ошибок нет.
fn dir_of(p: &std::path::Path) -> PathBuf {
    p.parent().map(PathBuf::from).unwrap_or_default()
}
