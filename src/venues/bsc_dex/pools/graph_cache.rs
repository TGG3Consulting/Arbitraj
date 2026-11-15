//! Файл: venues/bsc_dex/pools/graph_cache.rs
//! Что делает: кэш графа маршрутов (решение 129): граф строится один раз на состояние реестра (версия реестра) и
//! настройки графа (до решения 301 (е) — и на блок головы); все котировки и поиск циклов этого состояния берут готовый снимок (Arc). Одно построение
//! за раз (решение 131): параллельные котировки при новой версии реестра не строят один и тот же граф каждая сама.
//! Решение 189: здесь же — кэш НАБОРА путей графа (скелеты: какие пулы и в какой стороне, без состояния) по монете на
//! dex.route_paths_ttl_ms: граф по версии реестра пересобирается почти на каждой котировке (каждый своп любого пула
//! меняет версию), а набор путей за секунды подбора объёма не меняется. Состояние пулов котировка берёт свежим из
//! реестра (quote/route_cache.rs).
//! Решение 217 (исправление по проверке): у замороженного реестра набор путей закреплён (pinned) — пути, которые
//! живой кэш нашёл для монеты на момент снимка, без срока.
//! Решение 301 (задача t132, живой бот 15.11.2026): граф из кэша годится запросу, если он НЕ СТАРШЕ запроса — версия
//! реестра у графа не меньше запрошенной, настройки графа и USDT те же (Stamp::serves). Раньше нужно было
//! точное равенство: дождавшийся замка построения видел граф, построенный другим на более новой версии (пока ждал,
//! версия ушла), и строил ещё раз — очередь повторных построений (ожидание ≈ число ждущих × время построения).
//! Граф более новой версии — более свежие данные пулов (версия читается ДО чтения реестра), граф зависит только от
//! реестра, настроек и токенов: «не торгуем вслепую» не нарушается. Версия запроса выше, чем у графа, или другие
//! настройки — граф строится заново, как раньше. builds — счётчик построений (замер, без настроек).
//! Решение 301 (е): блока головы в ключе больше нет — build блок не читает (граф зависит только от реестра, настроек
//! графа и токенов; глубина пулов — pools/depth_book.rs — номера блока тоже не читает), поэтому граф не перестраивается
//! на каждом новом блоке, если реестр не менялся. Свежесть головы проверяют котировка и исполнение (quote/mod.rs,
//! verify.rs), а не граф.
//! Решение 301 (в): брошенный расчёт (quote/abandon.rs — срок расчёта вышел) не ждёт замок и не строит: признак
//! проверяется до ожидания замка и после его получения.
//! Решение 301 (д): замер построения (ожидание замка, очередь, build, N/R/E) — запись dex.graph_build по
//! log.graph_timing / log.graph_timing_min_ms (pools/graph_timing.rs).
//! Решение 308 (задача t137): при dex.graph_background = да и запущенном фоновом владельце (graph_owner.rs) cached отдаёт
//! последний опубликованный снимок (graph_read.rs) — без замка building, без построения в расчёте; путь ниже (кэш по
//! Stamp, одно построение за раз) остаётся при выключенной настройке и для реестра без владельца (замороженный —
//! решение 217). Здесь же — состояние владельца (bg) и настройки графа текстом (graph_settings — общие с basis снимка).
//! Кто вызывает: quote/graph_route.rs, quote/cycle_scan.rs (через graph::cached); pools/registry.rs (хранит Cache).
//! От чего зависит: graph.rs (build_measured), graph_timing.rs, quote/abandon.rs, registry.rs, tokens.rs, ctx.rs.
//! Настройки: dex.route_max_pools_per_edge, dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct
//! [bsc_dex] — часть ключа кэша; через graph_timing.rs — log.graph_timing, log.graph_timing_min_ms.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy_primitives::Address;

use crate::core::types::PoolRef;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::quote::abandon;
use super::super::tokens::Tokens;
use super::graph::{Graph, build_measured};
use super::graph_read;
use super::graph_snap::Bg;
use super::graph_timing::{self, Measure};
use super::registry::Registry;

/// На чём построен граф (решение 129): версия реестра, dex.route_max_pools_per_edge, настройки глубины (полоса, порог,
/// гистерезис — текстом), USDT и его decimals. Решение 301 (е): блока головы нет.
#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    version: u64,
    settings: Vec<String>,
    usdt: Option<Address>,
    usdt_dec: u8,
}

impl Stamp {
    /// Решение 301: годится ли граф, построенный на `self`, запросу `want` — те же настройки графа и USDT, версия
    /// реестра не меньше запрошенной (граф не старше запроса). Ошибок нет.
    fn serves(&self, want: &Stamp) -> bool {
        self.settings == want.settings
            && self.usdt == want.usdt
            && self.usdt_dec == want.usdt_dec
            && self.version >= want.version
    }
}

/// Кэш последнего графа (решение 129). Один на реестр (pools/registry.rs). Дешёво клонируется. `building` — одно
/// построение за раз: параллельные котировки при новой версии реестра не строят один и тот же граф каждая сама.
#[derive(Clone, Default)]
pub struct Cache {
    slot: Arc<Mutex<Option<Built>>>,
    building: Arc<Mutex<()>>,
    /// Решение 189: наборы путей по ключу (USDT, монета, настройки перебора) и время их построения.
    paths: Arc<Mutex<HashMap<PathsKey, Stored>>>,
    /// Решение 217 (исправление по проверке): у замороженного реестра — набор путей монеты из живого кэша на момент
    /// снимка, без срока (снимок не стареет); граф по реестру снимка не строится, пока ключ совпадает.
    pinned: Option<Arc<(PathsKey, Arc<Vec<Skeleton>>)>>,
    /// Решение 301: сколько раз граф строился (замер для проверки очереди построений; настроек нет).
    builds: Arc<AtomicU64>,
    /// Решение 301 (д): сколько построений сейчас ждут замок `building` (замер dex.graph_build).
    waiting: Arc<AtomicUsize>,
    /// Решение 308: фоновый владелец графа — снимок, изменённые пулы, счётчики (graph_snap.rs). У замороженного реестра
    /// (решение 217) — свой, без владельца.
    bg: Arc<Bg>,
}

/// Скелет пути (решение 189): по шагам — пул, токен входа, токен выхода. Без состояния пула.
pub type Skeleton = Vec<(PoolRef, Address, Address)>;

/// Набор путей и когда построен.
type Stored = (Instant, Arc<Vec<Skeleton>>);

/// Ключ набора путей: USDT, монета, настройки перебора текстом.
pub type PathsKey = (Address, Address, String);

impl Cache {
    /// Решение 217: кэш замороженного реестра с закреплённым набором путей (см. поле pinned). Ошибок нет.
    pub fn pinned(key: PathsKey, v: Arc<Vec<Skeleton>>) -> Cache {
        Cache {
            pinned: Some(Arc::new((key, v))),
            ..Cache::default()
        }
    }

    /// Закреплённый набор путей (решение 217), если ключ совпадает. Ошибок нет.
    pub fn paths_pinned(&self, key: &PathsKey) -> Option<Arc<Vec<Skeleton>>> {
        self.pinned
            .as_ref()
            .filter(|p| p.0 == *key)
            .map(|p| p.1.clone())
    }

    /// Решение 308: состояние фонового владельца графа этого реестра. Ошибок нет.
    pub fn bg(&self) -> &Arc<Bg> {
        &self.bg
    }

    /// Решение 301: сколько раз этот кэш строил граф. Ошибок нет.
    pub fn builds(&self) -> u64 {
        self.builds.load(Ordering::Relaxed)
    }

    /// Набор путей, если построен не раньше `ttl` назад. Ошибок нет.
    pub fn paths_get(&self, key: &PathsKey, ttl: Duration) -> Option<Arc<Vec<Skeleton>>> {
        let m = self.paths.lock().unwrap_or_else(|p| p.into_inner());
        m.get(key)
            .filter(|(at, _)| at.elapsed() <= ttl)
            .map(|(_, v)| v.clone())
    }

    /// Запомнить набор путей; заодно выбросить устаревшие (старше `ttl`). Ошибок нет.
    pub fn paths_put(&self, key: PathsKey, v: Vec<Skeleton>, ttl: Duration) {
        let mut m = self.paths.lock().unwrap_or_else(|p| p.into_inner());
        m.retain(|_, (at, _)| at.elapsed() <= ttl);
        m.insert(key, (Instant::now(), Arc::new(v)));
    }
}

/// Построенный граф и на чём он построен.
type Built = (Stamp, Arc<Graph>);

/// Граф текущего состояния: из кэша, если граф там не старше текущего реестра и построен на тех же настройках графа
/// (решение 301), иначе строится заново (build) и кладётся в кэш. Принимает: окружение, реестр, токены, знаки USDT.
/// Возвращает: граф. Ошибки: как у build; Timeout — расчёт брошен (abandon).
pub fn cached(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
) -> GatewayResult<Arc<Graph>> {
    // Решение 308: режим «граф без ожидания» и у реестра есть фоновый владелец — последний опубликованный снимок,
    // расчёт граф не строит и не ждёт (graph_read.rs). Иначе — путь t132 ниже.
    if graph_read::active(ctx, reg) {
        return graph_read::current(ctx, reg, tokens, usdt_dec);
    }
    // Версия — ДО чтения реестра: запись во время построения даст новую версию, и следующий вызов перестроит граф.
    cached_as_of(ctx, reg, tokens, usdt_dec, reg.version())
}

/// Настройки графа текстом (как прочитаны; не задана — пусто): dex.route_max_pools_per_edge, dex.depth_band_pct,
/// dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct — часть ключа кэша (решение 129) и basis снимка фонового графа
/// (решение 308, graph_snap.rs). Ошибок нет.
pub fn graph_settings(ctx: &Ctx) -> Vec<String> {
    let dec = |n: &str| {
        ctx.dec(n)
            .map(|d| d.normalize().to_string())
            .unwrap_or_default()
    };
    vec![
        ctx.int("dex.route_max_pools_per_edge")
            .map(|v| v.to_string())
            .unwrap_or_default(),
        dec("dex.depth_band_pct"),
        dec("dex.min_pool_liquidity_usd"),
        dec("dex.depth_hysteresis_pct"),
    ]
}

/// То же, что cached, для версии реестра, прочитанной вызывающим раньше (решение 301: так граф видит тот, кто ждал
/// замок построения, пока версия росла; этим же проверяется очередь построений — tests/bsc_dex_graph_queue.rs).
/// Принимает: окружение, реестр, токены, знаки USDT, версию реестра запроса. Возвращает: граф не старше
/// запроса. Ошибки: как у build; Timeout — расчёт брошен (quote/abandon.rs).
pub fn cached_as_of(
    ctx: &Ctx,
    reg: &Registry,
    tokens: &Tokens,
    usdt_dec: u8,
    version: u64,
) -> GatewayResult<Arc<Graph>> {
    // Значения как прочитаны (не задана — пусто; build сам скажет, какой нет).
    let stamp = Stamp {
        version,
        settings: graph_settings(ctx),
        usdt: tokens.usdt(ctx).ok(),
        usdt_dec,
    };
    let cache = reg.graph_cache();
    let hit = || {
        cache
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|(st, _)| st.serves(&stamp))
            .map(|(_, g)| g.clone())
    };
    if let Some(g) = hit() {
        return Ok(g);
    }
    // Решение 301 (в): срок расчёта вышел — замок не ждём.
    abandon::check(ctx, "ожидание построения графа")?;
    // Одно построение за раз; дождавшийся берёт граф, построенный другим, если он не старше запроса (решение 301).
    // Вызывается из счёта котировки вне рабочих потоков tokio (quote/cpu.rs) — ожидание замка рантайм не держит.
    // Решение 301 (д): замер очереди — сколько ждали при входе, сколько ждут за ним, сколько ждал он.
    let t0 = Instant::now();
    let ahead = cache.waiting.fetch_add(1, Ordering::AcqRel);
    let _one = cache.building.lock().unwrap_or_else(|p| p.into_inner());
    cache.waiting.fetch_sub(1, Ordering::AcqRel);
    let wait = t0.elapsed();
    let behind = cache.waiting.load(Ordering::Acquire);
    if let Some(g) = hit() {
        return Ok(g);
    }
    abandon::check(ctx, "построение графа")?;
    cache.builds.fetch_add(1, Ordering::Relaxed);
    let slot = &cache.slot;
    let t1 = Instant::now();
    let (g, stats) = build_measured(ctx, reg, tokens, usdt_dec)?;
    let g = Arc::new(g);
    graph_timing::report(
        ctx,
        &Measure {
            wait,
            ahead,
            behind,
            build: t1.elapsed(),
            stats,
            edges: g.len(),
        },
    );
    let mut m = slot.lock().unwrap_or_else(|p| p.into_inner());
    // Не затираем более новый граф, построенный параллельно.
    if m.as_ref().is_none_or(|(st, _)| st.version <= version) {
        *m = Some((stamp, g.clone()));
    }
    Ok(g)
}
