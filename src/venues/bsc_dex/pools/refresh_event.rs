//! Файл: venues/bsc_dex/pools/refresh_event.rs
//! Что делает: перечитывание пула ПО СОБЫТИЮ в сессии пачки (pools/listen_log.rs): полоса цены вышла за прочитанные
//! слова карты тиков (off_map, решение 73) и событие, требующее перечитать пул (Applied::Refresh: реорг, расхождение).
//! Чтение — load.rs refresh, как раньше. Решение 313: при log.pool_refresh_timing = да — замер (refresh_timing.rs):
//! общее время, каждое обращение к узлу («ожидание в очереди / сам запрос», node/req_trail.rs), слова карты, сколько
//! сообщений ждало в канале сессии к концу. Не задана / нет — тот же вызов load::refresh без замера.
//! Запись журнала «полоса цены вышла за прочитанные слова битмапа» (off_map) — здесь же, текст прежний.
//! Решение 314: чтение идёт классом очереди узла dex.event_refresh_urgency (refresh_urgency.rs) — только на время
//! чтения; не задана — класс прежний.
//! Решение 315: пул V3 — одним обращением к узлу (load_v3_once.rs) при dex.event_refresh_one_call = да и dex.multicall3;
//! иначе — load::refresh.
//! Решение 325: не применилось 315 — при dex.event_refresh_batch для DEX пула чтение с ответами, прочитанными заранее
//! (prefetch_read.rs: одно-два обращения к узлу вместо цепочки; тот же код чтения load::refresh, тот же итог).
//! Ревью 325 (Н4): «только комиссия» Algebra Integral (fee → integral_fee.rs apply) — тем же классом очереди и тем же
//! замером: запись dex.pool_refresh с причиной «fee» (слов карты — нет: карта не читается).
//! Кто вызывает: pools/listen_log.rs (ветки off_map, Refresh и Fee).
//! От чего зависит: load.rs (refresh), refresh_timing.rs, node/req_trail.rs, gateway/urgency.rs (класс для записи),
//! group.rs (Msg — канал сессии), integral_fee.rs (apply).
//! Настройки: log.pool_refresh_timing (через refresh_timing.rs), dex.event_refresh_urgency (через refresh_urgency.rs),
//! dex.event_refresh_one_call (через load_v3_once.rs), dex.event_refresh_batch (через prefetch_read.rs),
//! dex.event_refresh_fee_only (решает listen_log.rs через integral_fee.rs fits).

use std::time::Instant;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;
use crate::gateway::urgency;
use crate::storage::journal::Level;

use super::super::node::req_trail::{self, Trail};
use super::group::Msg;
use super::listen::Pools;
use super::refresh_timing::{self, Rec};
use super::refresh_urgency;
use super::registry::Entry;
use super::{integral_fee, load, load_v3_once, prefetch_read};

/// Что вызвало перечитывание.
pub struct Event<'a> {
    /// «off_map» — полоса глубины вышла за карту; «refresh» — событие требует перечитать пул; «fee» — только комиссия
    /// (ревью 325, fee).
    pub cause: &'static str,
    /// Блок события.
    pub block: u64,
    /// Канал сессии пачки (сколько сообщений ждёт — для замера).
    pub queued: &'a UnboundedReceiver<Msg>,
}

impl<'a> Event<'a> {
    /// Событие из причины, блока и канала сессии. Ошибок нет.
    pub fn new(cause: &'static str, block: u64, queued: &'a UnboundedReceiver<Msg>) -> Event<'a> {
        Event {
            cause,
            block,
            queued,
        }
    }
}

/// Полоса цены вышла за прочитанные слова карты (решение 73, depth::off_map): запись журнала, как раньше, и
/// перечитывание на блоке события (причина off_map). Принимает: окружение пачки, запись, блок события, канал сессии.
/// Возвращает: обновлённую запись. Ошибки: как у load::refresh.
pub async fn off_map(
    p: &Pools,
    e: &Entry,
    block: u64,
    queued: &UnboundedReceiver<Msg>,
) -> GatewayResult<Entry> {
    let msg = format!(
        "{} {}: полоса цены dex.depth_band_pct вышла за прочитанные слова битмапа — карта тиков перечитывается вокруг нового тика",
        e.family.title, e.ticker
    );
    p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    refresh(p, e, block, &Event::new("off_map", block, queued)).await
}

/// Перечитывает пул по событию на блоке `at`. Принимает: окружение пачки, запись пула, блок чтения, событие.
/// Возвращает: обновлённую запись (valid = true). Ошибки: как у load::refresh.
pub async fn refresh(p: &Pools, e: &Entry, at: u64, ev: &Event<'_>) -> GatewayResult<Entry> {
    // Решение 314: класс очереди узла — только на время этого чтения (refresh_urgency.rs); не задан — как раньше.
    let class = refresh_urgency::class(&p.ctx);
    if !refresh_timing::on(&p.ctx) {
        return refresh_urgency::with(class, read(p, e, at)).await;
    }
    let trail = Trail::default();
    let t0 = Instant::now();
    let res = req_trail::scope(trail.clone(), refresh_urgency::with(class, read(p, e, at))).await;
    let rec = Rec {
        entry: e,
        cause: ev.cause,
        block: ev.block,
        at,
        total: t0.elapsed(),
        steps: trail.take(),
        class: format!("{:?}", class.unwrap_or_else(urgency::current)),
        words: res
            .as_ref()
            .ok()
            .and_then(|x| x.ticks.as_ref())
            .map(|t| i32::from(t.words.1) - i32::from(t.words.0) + 1),
        queued: ev.queued.len(),
        err: res.as_ref().err().map(|x| x.message.clone()),
    };
    refresh_timing::report(&p.ctx, rec);
    res
}

/// Ревью 325 (Н4): только комиссия Algebra Integral на блоке события (integral_fee.rs apply) — классом
/// dex.event_refresh_urgency и с замером log.pool_refresh_timing (причина «fee», слов карты нет). Принимает: окружение
/// пачки, запись пула до события, ссылку, блок и logIndex события, канал сессии. Ошибки: как у integral_fee::apply.
pub async fn fee(
    p: &Pools,
    e: &Entry,
    r: &PoolRef,
    (block, index): (u64, Option<u64>),
    queued: &UnboundedReceiver<Msg>,
) -> GatewayResult<()> {
    let class = refresh_urgency::class(&p.ctx);
    let work = integral_fee::apply(p, r, block, index);
    if !refresh_timing::on(&p.ctx) {
        return refresh_urgency::with(class, work).await;
    }
    let trail = Trail::default();
    let t0 = Instant::now();
    let res = req_trail::scope(trail.clone(), refresh_urgency::with(class, work)).await;
    let rec = Rec {
        entry: e,
        cause: "fee",
        block,
        at: block,
        total: t0.elapsed(),
        steps: trail.take(),
        class: format!("{:?}", class.unwrap_or_else(urgency::current)),
        words: None,
        queued: queued.len(),
        err: res.as_ref().err().map(|x| x.message.clone()),
    };
    refresh_timing::report(&p.ctx, rec);
    res
}

/// Само чтение: решение 315 — пул V3 одним обращением к узлу (load_v3_once.rs), если включено и применимо; иначе
/// load::refresh (как до решения 313). Итог тот же: load::renewed на блоке чтения. Ошибки: как у load::refresh.
async fn read(p: &Pools, e: &Entry, at: u64) -> GatewayResult<Entry> {
    if let Some(l) = load_v3_once::read(&p.ctx, e, at).await? {
        return Ok(load::renewed(e, at, l));
    }
    // Решение 325: любой вид — с ответами, прочитанными заранее (prefetch_read.rs), при dex.event_refresh_batch.
    if let Some(x) = prefetch_read::refresh(&p.ctx, &p.tokens, &p.reg, e, at).await? {
        return Ok(x);
    }
    load::refresh(&p.ctx, &p.tokens, &p.reg, e, at).await
}
