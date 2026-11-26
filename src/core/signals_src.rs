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
use crate::storage::{db::Db, episodes_db, signals_archive, windows_db};

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

/// Читатель и границы автоматической выгрузки, включая учёт для привязки сделок к окнам.
struct ArchivePeriod<'a> {
    db: &'a Db,
    deals: Arc<Vec<Deal>>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

impl Core {
    /// Общий источник ручного отчёта; старые и открытые сводки читаются по прежним правилам.
    pub async fn signals_src(&self, now: DateTime<Utc>) -> Result<SignalsSrc, String> {
        self.signals_from(now, None).await
    }

    /// Суточная выборка сохраняет окна привязки сделок по обе стороны полуночи и эпизоды до фактического конца окна.
    pub(crate) async fn signals_src_archive(
        &self,
        db: &Db,
        deals: Arc<Vec<Deal>>,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<SignalsSrc, String> {
        self.signals_from(
            now,
            Some(ArchivePeriod {
                db,
                deals,
                from,
                to,
            }),
        )
        .await
    }

    /// Общее наложение открытых сводок. Разбор архива выполняется в блокирующем потоке.
    async fn signals_from(
        &self,
        now: DateTime<Utc>,
        archive: Option<ArchivePeriod<'_>>,
    ) -> Result<SignalsSrc, String> {
        let db = &self.inner.deps.db;
        let wrows = match &archive {
            Some(a) => signals_archive::windows(a.db, a.from, a.to).await,
            None => windows_db::oldest_first(db).await,
        }
        .map_err(|e| format!("окна: {e}"))?;
        let open_w = lk(&self.inner.windows).list();
        let open_e = lk(&self.inner.episodes).list();
        let windows = tokio::task::spawn_blocking(move || {
            wrows
                .into_iter()
                .filter_map(|b| serde_json::from_str::<WindowInfo>(&b).ok())
                .map(|w| match open_w.iter().find(|o| o.id == w.id) {
                    Some(o) => WindowInfo {
                        directions: o.directions.clone(),
                        venues: o.venues.clone(),
                        queue_traces: o.queue_traces.clone(),
                        max_profit: o.max_profit,
                        dex_points: o.dex_points.max(w.dex_points),
                        ..w
                    },
                    None => w,
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| e.to_string())?;
        let from = archive.as_ref().and_then(|a| a.from);
        let mut through = archive.as_ref().and_then(|a| a.to).unwrap_or(now);
        let mut explicit = std::collections::HashSet::new();
        let erows = match &archive {
            Some(a) => {
                let mut span: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
                for w in &windows {
                    if from.is_none_or(|f| w.started_at >= f)
                        && a.to.is_none_or(|t| w.started_at < t)
                    {
                        let end = w.ended_at.unwrap_or(now);
                        span = Some(span.map_or((w.started_at, end), |(a, b)| {
                            (a.min(w.started_at), b.max(end))
                        }));
                        explicit.extend(w.deals.iter().map(|d| d.id.clone()));
                    }
                }
                match span {
                    Some((first, last)) => {
                        through = through.max(last);
                        signals_archive::episodes(a.db, first, last).await
                    }
                    None => Ok(vec![]),
                }
            }
            None => episodes_db::oldest_first(db).await,
        }
        .map_err(|e| format!("эпизоды: {e}"))?;
        let archived = archive.map(|a| a.deals);
        let core = self.clone();
        let (episodes, deals) = tokio::task::spawn_blocking(move || {
            let episodes = erows
                .into_iter()
                .filter_map(|b| serde_json::from_str::<EpisodeInfo>(&b).ok())
                .map(|e| open_e.iter().find(|o| o.id == e.id).cloned().unwrap_or(e))
                .collect();
            let deals = match archived {
                Some(deals) => deals
                    .iter()
                    .filter(|d| {
                        explicit.contains(&d.id)
                            || (from.is_none_or(|f| d.started_at >= f) && d.started_at <= through)
                    })
                    .cloned()
                    .collect(),
                None => core.deals(),
            };
            (episodes, deals)
        })
        .await
        .map_err(|e| e.to_string())?;
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
            deals,
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
