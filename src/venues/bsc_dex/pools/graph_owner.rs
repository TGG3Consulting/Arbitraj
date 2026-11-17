//! Файл: venues/bsc_dex/pools/graph_owner.rs
//! Что делает: решение 308 (задача t137) — ФОНОВЫЙ ВЛАДЕЛЕЦ графа маршрутов (dex.graph_background = да): одна задача
//! tokio на реестр строит граф и публикует неизменяемый снимок (graph_snap.rs); расчёты граф не строят и не ждут
//! (проект docs/notes/graph-snapshot-design-2026-11-16.md).
//! Пробуждения — только по событию (И5): отметка изменённого пула реестром (graph_changes.rs, склейка всплеска),
//! изменение настроек графа (подписка хранилища, склейка пачки), запрос полной пересборки (кнопка, пересборка подписки
//! на рынок, запуск), окончание полной сборки, срок самопроверки dex.graph_verify_s. Опроса по таймеру нет.
//! На каждом пробуждении (graph_step.rs step): режим выключен — снимок снят, слежение выключено; настройки графа/USDT не готовы —
//! снимка нет с причиной; нет карты, сменились настройки или пришёл запрос — запуск полной сборки (graph_full.rs) в
//! отдельном потоке; есть изменённые пулы и карта на текущих настройках — накопительный круг (graph_round.rs) в потоке
//! блокирующих задач и публикация. Полная сборка идёт параллельно кругам; её окончание (finish) — доводка, сверка,
//! замена (самоисцеление), событие при расхождении.
//! Паника круга — выход с причиной; сторож (graph_guard.rs) перезапускает. Публикует только текущее поколение (generation).
//! Кто вызывает: parts.rs (ensure — при подписке DEX на рынок), graph_guard.rs (run).
//! От чего зависит: graph_step.rs (пробуждение, круг), graph_finish.rs (окончание полной сборки), graph_guard.rs,
//! graph_snap.rs, graph_work.rs, registry.rs, tokens.rs, ctx.rs, settings/live.rs (Watch).
//! Настройки: dex.graph_background, dex.graph_verify_s, dex.route_max_pools_per_edge [bsc_dex]; следит за dex.graph_*,
//! dex.route_max_pools_per_edge, dex.depth_band_pct, dex.min_pool_liquidity_usd, dex.depth_hysteresis_pct,
//! venue.quote_asset.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle};

use crate::core::types::PoolRef;
use crate::gateway::events::MarketEvent;
use crate::settings::live::Watch;

use super::super::ctx::Ctx;
use super::super::tokens::Tokens;
use super::graph_snap::Basis;
use super::graph_work::Work;
use super::registry::Registry;
use super::{graph_finish, graph_guard, graph_read, graph_step};

/// Причина выхода «система останавливается» — сторож не перезапускает.
pub const STOP: &str = "рассылка настроек закрыта — система останавливается";
/// Причина выхода «уже не текущее поколение» (повторное ревью п. 2) — читать её некому.
pub const ORPHAN: &str = "поколение владельца сменилось — владелец вышел";

/// Настройки, смена которых будит владельца (имена — те, от которых зависит снимок; как у resubscribe.rs).
/// Настройка, влияющая на курс встречного токена (ревью №1 P).
const FEE: &str = "dex.v2_fee_pct";

const WATCH: [&str; 10] = [
    "dex.graph_",
    "dex.route_max_pools_per_edge",
    "dex.depth_band_pct",
    "dex.min_pool_liquidity_usd",
    "dex.depth_hysteresis_pct",
    // Решение 390 (ревью t178, Н2): допуск мелкого пула в граф (quote/shallow_pick.rs).
    "dex.shallow_pools_per_pair",
    "check.probe_usdt",
    "dex.quoter_max_diff_pct",
    "venue.quote_asset",
    FEE,
];

/// Окружение владельца. Дешёво клонируется.
#[derive(Clone)]
pub struct Owner {
    pub ctx: Ctx,
    pub reg: Registry,
    pub tokens: Tokens,
    /// Поколение (сторож): публикует только текущее.
    pub generation: u64,
}

/// Идущая полная сборка.
pub(super) struct FullJob {
    pub(super) handle: JoinHandle<Result<(Work, usize), String>>,
    /// Пулы, оценённые кругами с начала сборки (для доводки).
    pub(super) since: HashSet<PoolRef>,
    pub(super) reason: String,
    pub(super) started: Instant,
    pub(super) basis: Basis,
    /// Ревью №2 (B): отмена — сборка заменена новой или режим выключен; поток останавливается на ближайшем пуле.
    pub(super) cancel: Arc<AtomicBool>,
}

impl FullJob {
    /// Отменить сборку (поток остановится на ближайшем пуле, итог не нужен). Ошибок нет.
    pub(super) fn stop(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Состояние владельца.
pub(super) struct St {
    pub(super) work: Option<Work>,
    pub(super) full: Option<FullJob>,
    pub(super) next_verify: Option<tokio::time::Instant>,
    /// Ревью №2 (G): значение dex.graph_verify_s, по которому назначен срок (сменилось — срок назначается заново).
    pub(super) verify_s: Option<u64>,
    /// Ревью №2 (E): запрос полной пересборки, пришедший во время идущей сборки, — после неё.
    pub(super) again: Option<String>,
}

/// Итог потока полной сборки: карта и пулов в реестре, или причина; JoinError — поток упал.
pub(super) type FullResult = Result<Result<(Work, usize), String>, JoinError>;

/// Что разбудило владельца.
enum Ev {
    Wake,
    /// Изменились настройки; true — среди них dex.v2_fee_pct (ревью №1 P).
    Settings(bool),
    Stop,
    /// За Box: итог сборки (карта) много больше остальных вариантов (clippy::large_enum_variant).
    Full(Box<FullResult>),
    Verify,
}

/// Запуск владельца у реестра при подписке DEX на рынок (Parts::pools): первый раз — сторож с владельцем; повторно
/// (пересборка подписки — ленты пулов переподключены) — запрос полной пересборки. Принимает: окружение, реестр,
/// токены, канал событий ядру. Ошибок нет.
pub fn ensure(ctx: &Ctx, reg: &Registry, tokens: &Tokens, out: mpsc::Sender<MarketEvent>) {
    let bg = reg.graph_cache().bg();
    bg.set_out(out);
    // Вне среды tokio (синхронный вызов) запускать некуда — владельца нет, работает прежний путь t132.
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if !bg.start_once() {
        bg.request_full("пересборка подписки DEX на рынок — ленты пулов переподключены");
        return;
    }
    rt.spawn(graph_guard::keep(ctx.clone(), reg.clone(), tokens.clone()));
}

/// Цикл владельца (см. шапку). Принимает: окружение. Возвращает: причину выхода (STOP — система останавливается;
/// иначе — сбой, сторож перезапустит). Ошибок нет.
pub async fn run(o: Owner) -> String {
    let bg = o.reg.graph_cache().bg().clone();
    let mut watch = Watch::new(
        o.ctx.settings.subscribe(),
        WATCH.iter().map(|p| p.to_string()).collect(),
    );
    let mut st = St {
        work: None,
        full: None,
        next_verify: None,
        verify_s: None,
        again: None,
    };
    // Третье ревью п. 5: первая сборка — без запроса (карты нет — сборка нужна и так), поэтому сохранённый в Bg срок
    // повтора неудавшейся сборки она не обходит.
    loop {
        // Повторное ревью п. 2: сторож (или новый сторож после паники прежнего) запустил другого владельца — этот выходит
        // без побочных действий; идущая у него сборка останавливается сама (поколение уже не текущее).
        if !bg.is_current(o.generation) {
            return ORPHAN.to_string();
        }
        if let Err(why) = graph_step::step(&o, &bg, &mut st).await {
            return why;
        }
        let ev = tokio::select! {
            _ = bg.changes.notified() => Ev::Wake,
            _ = bg.rebuild_notified() => Ev::Wake,
            r = watch.next() => match r {
                Some(c) => {
                    let mut fee = c.name.starts_with(FEE);
                    while let Some(c) = watch.try_next() {
                        fee |= c.name.starts_with(FEE);
                    }
                    Ev::Settings(fee)
                }
                None => Ev::Stop,
            },
            r = full_done(&mut st.full) => Ev::Full(Box::new(r)),
            _ = sleep_until(st.next_verify) => Ev::Verify,
            _ = sleep_until(bg.retry_at()) => Ev::Wake,
        };
        match ev {
            Ev::Wake => {}
            // Ревью №2 (G): срок самопроверки сбрасывается только при смене самой dex.graph_verify_s (graph_step.rs).
            Ev::Settings(fee_changed) => {
                // Ревью №1 (P): dex.v2_fee_pct входит в курс встречного токена (pool_quote::v2_fee_pct → rate::best_in), а не в
                // ключ книги глубины: смена — переводы в USD пересчитываются (Book::invalidate) и полная пересборка.
                // Только в режиме dex.graph_background = да: при выключенном книга глубины — как в t132.
                if fee_changed && graph_read::background(&o.ctx) {
                    o.reg.depth().invalidate();
                    bg.request_full("смена dex.v2_fee_pct — курс встречного токена");
                }
            }
            Ev::Stop => return STOP.to_string(),
            Ev::Full(r) => {
                if let Err(why) = graph_finish::finish(&o, &bg, &mut st, *r).await {
                    return why;
                }
            }
            Ev::Verify => {
                st.next_verify = None;
                bg.request_full("срок самопроверки dex.graph_verify_s");
            }
        }
    }
}

/// Окончание полной сборки (None — сборки нет: ждать вечно). Ошибок нет.
async fn full_done(f: &mut Option<FullJob>) -> FullResult {
    match f.as_mut() {
        Some(j) => (&mut j.handle).await,
        None => std::future::pending().await,
    }
}

/// Срок самопроверки (None — не задан: ждать вечно). Ошибок нет.
async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}
