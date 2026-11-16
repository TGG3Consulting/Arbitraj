//! Файл: venues/bsc_dex/pools/refresh_event.rs
//! Что делает: перечитывание пула ПО СОБЫТИЮ в сессии пачки (pools/listen_log.rs): полоса цены вышла за прочитанные
//! слова карты тиков (off_map, решение 73) и событие, требующее перечитать пул (Applied::Refresh: реорг, расхождение).
//! Чтение — load.rs refresh, как раньше. Решение 313: при log.pool_refresh_timing = да — замер (refresh_timing.rs):
//! общее время, каждое обращение к узлу («ожидание в очереди / сам запрос», node/req_trail.rs), слова карты, сколько
//! сообщений ждало в канале сессии к концу. Не задана / нет — тот же вызов load::refresh без замера.
//! Запись журнала «полоса цены вышла за прочитанные слова битмапа» (off_map) — здесь же, текст прежний.
//! Кто вызывает: pools/listen_log.rs (ветки off_map и Refresh).
//! От чего зависит: load.rs (refresh), refresh_timing.rs, node/req_trail.rs, gateway/urgency.rs (класс для записи),
//! group.rs (Msg — канал сессии).
//! Настройки: log.pool_refresh_timing (через refresh_timing.rs).

use std::time::Instant;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::gateway::errors::GatewayResult;
use crate::gateway::urgency;
use crate::storage::journal::Level;

use super::super::node::req_trail::{self, Trail};
use super::group::Msg;
use super::listen::Pools;
use super::load;
use super::refresh_timing::{self, Rec};
use super::registry::Entry;

/// Что вызвало перечитывание.
pub struct Event<'a> {
    /// «off_map» — полоса глубины вышла за карту; «refresh» — событие требует перечитать пул.
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
    if !refresh_timing::on(&p.ctx) {
        return read(p, e, at).await;
    }
    let trail = Trail::default();
    let t0 = Instant::now();
    let res = req_trail::scope(trail.clone(), read(p, e, at)).await;
    let rec = Rec {
        entry: e,
        cause: ev.cause,
        block: ev.block,
        at,
        total: t0.elapsed(),
        steps: trail.take(),
        class: format!("{:?}", urgency::current()),
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

/// Само чтение: load::refresh (как до решения 313). Ошибки: как у load::refresh.
async fn read(p: &Pools, e: &Entry, at: u64) -> GatewayResult<Entry> {
    load::refresh(&p.ctx, &p.tokens, &p.reg, e, at).await
}
