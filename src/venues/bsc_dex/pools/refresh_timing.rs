//! Файл: venues/bsc_dex/pools/refresh_timing.rs
//! Что делает: решение 313 — замер перечитывания пула по событию (полоса цены вышла за карту тиков — off_map; событие
//! требует перечитать — refresh; pools/listen_log.rs через refresh_event.rs): одна запись журнала `dex.pool_refresh`
//! на перечитывание — DEX, пул, причина, блок события и блок чтения, общее время, каждое обращение к узлу отдельно
//! («ожидание в очереди узла / сам запрос», число вызовов в нём — node/req_trail.rs), класс очереди узла, число слов
//! карты тиков, сколько сообщений ждало в канале сессии пачки к концу перечитывания, итог (ошибка — её текст).
//! Включаемый: log.pool_refresh_timing = да. Нет или не задана — замера и записи нет (одно чтение настройки на
//! перечитывание по событию). Запись — отдельной задачей tokio (сессия пачки её не ждёт).
//! Кто вызывает: pools/refresh_event.rs.
//! От чего зависит: ctx.rs (настройки, журнал), node/req_trail.rs (Step), registry.rs (Entry), depth_book.rs (label).
//! Настройки: log.pool_refresh_timing (вся система; необязательная).

use std::time::Duration;

use serde_json::json;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::req_trail::Step;
use super::depth_book::label;
use super::registry::Entry;

/// Включён ли замер: log.pool_refresh_timing = да. Не задана или нет — нет. Ошибок нет.
pub fn on(ctx: &Ctx) -> bool {
    ctx.flag("log.pool_refresh_timing").unwrap_or(false)
}

/// Замер одного перечитывания по событию.
pub struct Rec<'a> {
    /// Запись пула до перечитывания (DEX, тикер, ссылка).
    pub entry: &'a Entry,
    /// «off_map» или «refresh».
    pub cause: &'static str,
    /// Блок события и блок чтения.
    pub block: u64,
    pub at: u64,
    /// Всё перечитывание.
    pub total: Duration,
    /// Обращения к узлу по порядку.
    pub steps: Vec<Step>,
    /// Класс очереди узла, с которым шло перечитывание (gateway/urgency.rs).
    pub class: String,
    /// Слов карты тиков в прочитанной записи (None — карты нет или чтение не удалось).
    pub words: Option<i32>,
    /// Сообщений в канале к концу перечитывания: без полос — канал сессии пачки (все её пулы); при полосах решения 330
    /// (dex.event_lanes_max) — полоса этого пула (только его события). Ревью t153 (Н8): смысл разный, не сравнивать.
    pub queued: usize,
    /// Ошибка чтения (None — прочитано).
    pub err: Option<String>,
}

/// Миллисекунды с долями. Ошибок нет.
fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Запись замера в журнал (dex.pool_refresh). Вне среды tokio — без записи. Ошибок нет.
pub fn report(ctx: &Ctx, r: Rec<'_>) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let e = r.entry;
    let steps: Vec<String> = r
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}) очередь {:.1} мс, запрос {:.1} мс (вызовов {})",
                i + 1,
                ms(s.wait),
                ms(s.send),
                s.calls
            )
        })
        .collect();
    let words = r.words.map_or("нет".to_string(), |w| w.to_string());
    let outcome = r
        .err
        .as_deref()
        .map_or("прочитано".to_string(), |x| format!("ошибка: {x}"));
    let msg = format!(
        "{} {} ({}): перечитывание по событию ({}) — блок события {}, чтение на блоке {}: всего {:.1} мс; обращений \
         к узлу {} (класс очереди {}): {}; слов карты {words}; в канале сессии ждут {}; {outcome}",
        e.family.title,
        e.ticker,
        label(&e.pool.reference),
        r.cause,
        r.block,
        r.at,
        ms(r.total),
        r.steps.len(),
        r.class,
        steps.join("; "),
        r.queued
    );
    let data = json!({
        "dex": e.family.id,
        "pool": label(&e.pool.reference),
        "ticker": e.ticker,
        "cause": r.cause,
        "block": r.block,
        "at": r.at,
        "total_ms": ms(r.total),
        "class": r.class,
        "steps": r.steps.iter().map(|s| json!({
            "calls": s.calls,
            "wait_ms": ms(s.wait),
            "send_ms": ms(s.send),
        })).collect::<Vec<_>>(),
        "words": r.words,
        "queued": r.queued,
        "error": r.err,
    });
    let ctx = ctx.clone();
    rt.spawn(async move {
        ctx.log(Level::Info, "dex.pool_refresh", msg, Some(data))
            .await;
    });
}
