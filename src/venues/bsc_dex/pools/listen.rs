//! Файл: venues/bsc_dex/pools/listen.rs
//! Что делает: живое состояние пачки пулов: подписка logs → блок B того же узла WebSocket (eth_blockNumber по тому же
//! соединению, после подписки: всё, что позже B, придёт в подписку) → ждём, пока узел HTTP дойдёт до B → чтение
//! состояния на блоке B по HTTP → применение событий после B по порядку (blockNumber, logIndex); пулы — ядру.
//! Блок берётся у узла WS, а не HTTP: если HTTP отстаёт, события между его блоком и подпиской потерялись бы.
//! Реорг (removed: true) или расхождение события с картой — пул перечитывается. Обрыв — пулы пачки недостоверны
//! (событие Invalidated по монетам), пауза venue.ws_reconnect_pause_ms, всё заново.
//! Сессия (подписки пачки на одном соединении, чтение, события, мелкие пулы, приём новых пулов) — pools/session.rs;
//! журнал связи — pools/link.rs.
//! Кто вызывает: pools/market.rs, pools/created.rs (новые пулы).
//! От чего зависит: ctx.rs, node/ws.rs, node/rpc.rs, pools/load.rs, listen_log.rs (один лог), session.rs, link.rs, registry.rs, discover.rs.
//! Настройки: venue.ws_reconnect_pause_ms, feed.stale_after_ms [bsc_dex] (сколько ждать узел HTTP); через
//! вызываемые — узел, dex.*.
//!
//! «Removed logs (due to chain reorgs) are pushed with removed: true» —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60 ;
//! подписки привязаны к соединению — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub .

use std::time::Duration;

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;

use super::super::check::state::Checks;
use super::super::ctx::Ctx;
use super::super::node::heads::Heads;
use super::super::tokens::Tokens;
use super::discover::Found;
use super::mids::Mids;
use super::progress::Signals;
use super::registry::Registry;
use super::{link, session};

/// Окружение слушателей пулов. Дешёво клонируется.
#[derive(Clone)]
pub struct Pools {
    pub ctx: Ctx,
    pub reg: Registry,
    pub tokens: Tokens,
    pub out: mpsc::Sender<MarketEvent>,
    /// Итоги проверок монет и пулов.
    pub checks: Checks,
    /// Последний блок сети.
    pub heads: Heads,
    /// Промежуточные монеты (решение 52).
    pub mids: Mids,
    /// Стартовая загрузка, «пул готов», журнал без спама (pools/progress.rs).
    pub signals: Signals,
    /// Пулы, ждущие первого чтения — для «пулы найдены, загружаются» в ядре (loading.rs, решение 61).
    pub loading: super::loading::Loading,
    /// Лента квитанций блоков — источник событий пулов при bsc.pool_events_source = receipts (решение 68).
    pub feed: super::super::node::receipts::Feed,
    /// «Данные пулов отстают» (решение 72): проверки вердикт не выносят, пока отметка стоит.
    pub lag: super::super::node::events_lag::EventsLag,
}

/// Чем кончилась сессия.
pub enum End {
    /// Ядро закрыло канал событий — слушать больше некому.
    Closed,
    /// Обрыв или ошибка — переподключиться.
    Broken(String),
}

/// Слушает пачку пулов постоянно. Принимает: окружение, имя, найденные пулы с тикерами, канал пулов, найденных позже
/// (attach.rs; у стартовых — None):
/// они принимаются в идущую сессию и остаются в пачке после переподключения.
/// Обрыв после загрузки — пулы недостоверны (Invalidated) до перечитывания; сбой ещё до загрузки (подписка, узел HTTP
/// отстал) — пулы не трогаются (они либо ещё не прочитаны, либо уже недостоверны), повтор после паузы. В журнал — одна
/// запись на вид причины. Ошибок не возвращает; не задана пауза переподключения — ждёт её задания (settings_wait.rs).
pub async fn run(p: Pools, name: String, pools: Vec<(Found, String)>, adds: Option<session::Adds>) {
    let (mut pools, mut adds) = (pools, adds);
    loop {
        let (end, loaded) = session::run(&p, &name, &mut pools, &mut adds).await;
        let why = match end {
            End::Closed => return,
            End::Broken(why) => why,
        };
        // Пулы уже недостоверны, если загрузка была (session.rs — до закрытия «соединения»).
        // Журнал — одна запись на площадку при потере связи и одна при восстановлении (link.rs).
        link::down(&p, &name, &why, loaded).await;
        // Не задана — пачка ждёт настройку и переподключается сама (settings_wait.rs), а не останавливается навсегда.
        let what = format!("Пулы {name}");
        let read = || p.ctx.int("venue.ws_reconnect_pause_ms");
        let Some(ms) = super::super::settings_wait::until_ok(&p.ctx, &what, "venue.ws", read).await
        else {
            return;
        };
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}
