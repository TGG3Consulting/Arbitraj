//! Файл: venues/bsc_dex/pools/session.rs
//! Что делает: одна сессия слушателя пачки пулов (pools/listen.rs) на одном соединении WebSocket: приём всех пулов
//! пачки (admit.rs: подписка → блок B того же узла WS → чтение на B → глубокие и мелкие), затем события до обрыва.
//! Пулы, найденные позже, принимаются в идущую сессию тем же соединением (attach.rs шлёт их в канал пачки).
//! Не прочитавшийся пул перечитывается на новых блоках (reload.rs); брошенный (попытки исчерпаны) — снова, когда
//! задали настройку, из-за которой он не прочитался (revive.rs, решения 61, 63). Смена порога или полосы глубины —
//! пересчёт вердикта пулов сразу (depth_sync.rs). Связь с узлом WS — pools/link.rs.
//! Решение 298: перечитывание пула по событию опоздало (у узла нет состояния блока) — пул в очередь повтора на новых
//! блоках (stale_read.rs), без обрыва сессии.
//! Решение 295: лог контракта пар многомонетного пула (PoolRef::Multi) — listen_site.rs (одна перечитка на контракт).
//! Решение 330: при dex.event_lanes_max события глубоких пулов (свои Contract / Singleton, все виды) — полосами
//! (session_lanes.rs): перечитывание одного пула не держит события других пулов пачки, порядок событий пула прежний.
//! Новый блок ждёт только полосы своих пулов (повтор, «каждым блоком», «в обновлении» решения 357), приём пулов /
//! пересинхрон / настройки — всё принятое. Не задана — события по очереди в сессии, как раньше.
//! Ревью t153: Н1 — на обрыве полосы останавливаются и дожидаются остановки ДО invalidate (listen → events); Б1 —
//! итог true события пула ставит его в очередь повтора через pool_fail::requeue (pool_fail_note.rs).
//! Решение 357: новая голова закрывает пулы «в обновлении», чьи логи блока не пришли (settle_close.rs) — после
//! settle_block (полосы этих пулов дождались), «в пути» — канал пачки плюс события полосы пула.
//! Решение 455: лог ERC-20 Transfer на адрес crypto-пула Curve пачки (вторая подписка «все события», group.rs) —
//! отметка transfer_in.rs; голова читает такие пулы, если перевод не учтён чтением (per_block.rs).
//! Кто вызывает: pools/listen.rs.
//! От чего зависит: group.rs, admit.rs, reload.rs, listen_log.rs, events.rs, node/heads.rs, link.rs, settings (рассылка),
//! session_lanes.rs (решение 330), stale_read.rs.
//! Настройки: через вызываемые (feed.stale_after_ms, dex.pool_load_retries, dex.min_pool_liquidity_usd,
//! dex.event_lanes_max).
//!
//! Подписки привязаны к соединению — https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub .

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;

use alloy::rpc::types::Log;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use crate::core::types::PoolRef;

use super::admit::{self, Members};
use super::discover::Found;
use super::events;
use super::group::{self, Group, Msg};
use super::listen::{End, Pools};
use super::revive;
use super::session_lanes::EventLanes;
use super::{link, listen_log, listen_site, per_block, reload};

/// Канал пулов, найденных после старта пачки.
pub type Adds = mpsc::UnboundedReceiver<Vec<(Found, String)>>;

/// Сессия (см. шапку). Принимает: окружение, имя, пулы пачки (пополняются), канал новых пулов.
/// Возвращает: конец и была ли загрузка состояния.
pub async fn run(
    p: &Pools,
    name: &str,
    pools: &mut Vec<(Found, String)>,
    adds: &mut Option<Adds>,
) -> (End, bool) {
    if let Some(rx) = adds.as_mut() {
        while let Ok(more) = rx.try_recv() {
            extend(pools, more);
        }
    }
    let mut g = match Group::open(p, name).await {
        Ok(g) => g,
        Err(e) => return (End::Broken(e.message), false),
    };
    let mut m = Members {
        pools: std::mem::take(pools),
        deep: Vec::new(),
        missing: Vec::new(),
        abandoned: Vec::new(),
        loaded: false,
    };
    let all: Vec<usize> = (0..m.pools.len()).collect();
    let end = match admit::admit(p, &mut g, &mut m, all).await {
        Ok(()) => {
            link::up(p, name).await;
            p.signals.pool_ready();
            listen(p, &mut g, &mut m, adds).await
        }
        Err(end) => (end, m.loaded),
    };
    *pools = m.pools;
    // Обрыв после загрузки — пулы недостоверны (Invalidated) ДО закрытия «соединения» `g` (оно закрывается при выходе):
    // «сброс» ленты квитанций снимается, только когда закрыты все её «соединения» (node/receipts.rs, ревью #62).
    // dex.keep_pools_on_break = да (решение 93): на коротком обрыве монеты НЕ выкидываются — держим последнее
    // состояние пулов и переподключаемся (резервы не портятся от короткого разрыва; частые обрывы бесплатного WS
    // иначе дают флап — монеты пропадают и возвращаются). Сделки на устаревшем всё равно не проходят:
    // отставание ленты (решение 72) и предторговая верификация на свежем состоянии (решение 83). Не задано/нет —
    // прежнее поведение: обрыв = Invalidated по монетам пачки.
    if let (End::Broken(_), true) = &end
        && !p.ctx.flag("dex.keep_pools_on_break").unwrap_or(false)
    {
        listen_log::invalidate(p, pools);
    }
    end
}

/// События пачки до обрыва. Ревью t153 (Н1): полосы событий (решение 330) останавливаются и ДОЖИДАЮТСЯ остановки до
/// возврата — run объявляет пачку недостоверной (invalidate) только после этого. Возвращает: (конец, true —
/// состояние загружалось).
async fn listen(p: &Pools, g: &mut Group, m: &mut Members, adds: &mut Option<Adds>) -> (End, bool) {
    // Решение 330: полосы событий глубоких пулов (session_lanes.rs); без dex.event_lanes_max пусты — всё как раньше.
    let mut lanes = EventLanes::new();
    let out = Box::pin(events(p, g, m, adds, &mut lanes)).await;
    lanes.shutdown().await;
    out
}

/// События пачки до обрыва (тело listen). Принимает ещё полосы событий. Возвращает: (конец, true — состояние
/// загружалось).
async fn events(
    p: &Pools,
    g: &mut Group,
    m: &mut Members,
    adds: &mut Option<Adds>,
    lanes: &mut EventLanes,
) -> (End, bool) {
    // Источник могли сменить, пока шёл приём пулов (admit) — сразу переподключение через новый (решение 68).
    if g.conn.stale(&p.ctx) {
        return (
            End::Broken("источник событий пулов сменён (bsc.pool_events_source)".into()),
            true,
        );
    }
    let mut heads = p.heads.watch();
    // Изменения настроек: могла появиться недостающая (dex.load_batch и т.п.) — брошенные и ждущие пулы читаются
    // снова без переподключения и перезапуска.
    let (mut settings, mut open) = (p.ctx.settings.subscribe(), true);
    // Пропуск блока лентой (решение 77): перечитать состояние своих пулов на голове без разрыва подписки.
    let mut resync = p.feed.resync_watch();
    let mut idx = group::index(&m.pools);
    let mut sites = group::multi_sites(&m.pools);
    // Решение 357 (ревью слияния, Н3): самый поздний блок принятых из канала логов глубоких пулов — приняв лог блока
    // > B, сессия приняла все логи блока B подписки «все события» (settle_close.rs шапка).
    let mut seen: u64 = 0;
    loop {
        tokio::select! {
            done = lanes.next_done(), if lanes.running() => {
                let Some(done) = done else { continue };
                if let Err(end) = lanes.finish(m, done) {
                    return (end, true);
                }
                if let Err(end) = lanes.dispatch(p, m) {
                    return (end, true);
                }
            }
            msg = g.rx.recv() => {
                let raw = match msg {
                    Some(Msg::Log(raw)) => raw,
                    Some(Msg::Closed) | None => break,
                };
                let log: Log = match serde_json::from_str(raw.get()) {
                    Ok(l) => l,
                    Err(e) => return (End::Broken(format!("лог не разобран: {e}")), true),
                };
                if let Some(b) = log.block_number
                    && deep_log(m, &idx, &log)
                {
                    seen = seen.max(b);
                }
                if let Err(end) = on_log(p, g, m, (&idx, &sites), &log, (&mut *lanes, raw)).await {
                    return (end, true);
                }
            }
            more = next_add(adds) => {
                // Решение 330: приём новых пулов — после всего принятого полосами (как раньше по очереди).
                if let Err(end) = lanes.drain(p, m).await {
                    return (end, true);
                }
                let start = m.pools.len();
                extend(&mut m.pools, more);
                let new: Vec<usize> = (start..m.pools.len()).collect();
                if let Err(end) = admit::admit(p, g, m, new).await {
                    return (end, true);
                }
                idx = group::index(&m.pools);
                sites = group::multi_sites(&m.pools);
            }
            changed = resync.changed() => {
                if changed.is_err() {
                    return (End::Broken("лента остановлена".into()), true);
                }
                let epoch = *resync.borrow_and_update();
                if epoch > 0 {
                    // Решение 330: пересинхрон пачки — после всего принятого полосами.
                    if let Err(end) = lanes.drain(p, m).await {
                        return (end, true);
                    }
                    if let Err(end) = super::depth_sync::resync(p, m).await {
                        return (end, true);
                    }
                    // Пачка перечитала пулы — отметить «соединение» ленты (снимет запрет котировок, когда все догнали).
                    g.conn.resynced(epoch);
                }
            }
            // Решение 357: и когда есть пулы «в обновлении» — их закрывает голова (settle_close.rs).
            // Решение 453: и когда есть crypto-пулы Curve в рампе A/gamma или остановленные tricrypto 2021
            // (per_block::any_ramping); решение 455 — и пулы с переводом токена без события пула (transfer_in::any).
            changed = heads.changed(), if !m.missing.is_empty() || per_block::any(&m.pools) || p.holds.any()
                || per_block::any_ramping(p, &m.pools) || super::transfer_in::any(p, &m.pools) => {
                if changed.is_err() {
                    return (End::Broken("поток новых блоков остановлен".into()), true);
                }
                let at = heads.borrow_and_update().as_ref().map(|h| h.number);
                // Решение 330: повтор и чтения каждым блоком не идут одновременно с полосой СВОЕГО пула (одна запись
                // реестра — один читатель); полосы остальных пулов идут дальше. Решение 357: и полосы пулов «в обновлении».
                if let Err(end) = lanes.settle_block(p, m).await {
                    return (end, true);
                }
                // Решение 357: логи блока пула «в обновлении» не пришли к голове — полное чтение пула на голове (до
                // повтора: не прочитавшийся встаёт в очередь повтора). Ревью слияния (Н3): «в пути» — события полосы
                // ЭТОГО пула, а канал пачки — только пока сессия не приняла лог более позднего блока (settle_close.rs).
                let in_chan = g.rx.len();
                let transit = |i: usize, block: u64| {
                    super::settle_close::in_transit(lanes.queued(i), in_chan, seen, block)
                };
                if let Some(at) = at
                    && let Err(end) = super::settle_close::on_head(p, m, at, transit).await
                {
                    return (end, true);
                }
                if let Some(at) = at
                    && !m.missing.is_empty()
                    && let Err(end) = reload::retry(p, &m.pools, &mut m.missing, &mut m.abandoned, at).await
                {
                    return (end, true);
                }
                // Решение 122 F, 3: пулы, чьё состояние меняет оракул без событий (WooFi, Lista), — на каждом блоке.
                if let Some(at) = at
                    && let Err(end) = per_block::on_head(p, &m.pools, at).await
                {
                    return (end, true);
                }
                // Решение 330: ждавшие, пока шло чтение блока, — в работу.
                if let Err(end) = lanes.dispatch(p, m) {
                    return (end, true);
                }
            }
            changed = settings.recv(), if open => {
                // Сменили источник событий пулов (ws ↔ receipts, решение 68) — переподключение через новый.
                if g.conn.stale(&p.ctx) {
                    return (End::Broken("источник событий пулов сменён (bsc.pool_events_source)".into()), true);
                }
                // Решение 330: возврат брошенных и пересчёт глубины — после всего принятого полосами.
                if let Err(end) = lanes.drain(p, m).await {
                    return (end, true);
                }
                // Брошенные из-за настройки — revive.rs (рассылка закрыта — open = false); порог или полоса глубины —
                // пересчёт вердикта пулов пачки сразу (depth_sync.rs, решение 73).
                let depth_changed = super::depth_sync::touches(&changed);
                match revive::on_recv(p, &m.pools, &mut m.missing, &mut m.abandoned, changed).await {
                    Ok(still) => open = still,
                    Err(end) => return (end, true),
                }
                if depth_changed && let Err(end) = super::depth_sync::on_change(p, g, m).await {
                    return (end, true);
                }
            }
        }
    }
    let why = g
        .conn
        .end_reason()
        .unwrap_or_else(|| "подписка закрыта".into());
    (End::Broken(why), true)
}

/// Один лог: глубокий пул — событие его состояния (решение 330: при полосах — в полосу пула, session_lanes.rs);
/// мелкий — пробуждение. Принимает ещё полосы и сырой лог (для полосы). Ошибки: End — как у on_log и admit::wake.
async fn on_log(
    p: &Pools,
    g: &mut Group,
    m: &mut Members,
    (index, sites): (&HashMap<PoolRef, usize>, &HashSet<Address>),
    log: &Log,
    (lanes, raw): (&mut EventLanes, Box<RawValue>),
) -> Result<(), End> {
    // Решение 455: ERC-20 Transfer на адрес crypto-пула Curve пачки — отметка, чтение пула на голове (transfer_in.rs).
    if super::transfer_in::on_log(p, &m.pools, log) {
        return Ok(());
    }
    let single = log.topics().get(1).map(|id| PoolRef::Singleton {
        manager: log.address(),
        id: *id,
    });
    let found = single
        .and_then(|r| index.get(&r))
        .or_else(|| index.get(&PoolRef::Contract(log.address())));
    // Ревью 295: в пачке и Contract(адрес), и пары Multi того же адреса (смена dex.multi_pair на лету: старая подписка
    // ещё жива, index_watch уже подключил пары нового вида в её позднюю пачку — attach.rs) — пары Multi тоже получают
    // лог. У пачек без пар Multi множество пусто — путь прежний.
    if found.is_some() && sites.contains(&log.address()) {
        listen_site::on_log(p, g, m, log).await?;
    }
    let Some(&i) = found else {
        // Решение 295: пары многомонетного пула (PoolRef::Multi) — одна перечитка всех пар контракта (listen_site.rs).
        if listen_site::on_log(p, g, m, log).await? {
            return Ok(());
        }
        // Пары общего контракта (WooFi, PoolRef::Member): лог от адреса контракта — перечитать все его пары.
        // Решение 314: пары общего контракта по логу — классом dex.event_refresh_urgency (не задана — как раньше);
        // ревью 314: класс ставит per_block::on_member_log, только если пары у контракта есть (настройка не читается
        // на каждом логе, не нашедшем пул).
        per_block::on_member_log(p, &m.pools, log).await?;
        return Ok(());
    };
    if m.deep.get(i).copied().unwrap_or(true) {
        // Событие только для мелких (V2 Mint) — у глубокого пула состояние ведёт Sync.
        let family = m.pools[i].0.family;
        if log
            .topic0()
            .is_some_and(|t| events::topics(family).contains(t))
        {
            // Решение 330: полосы — событие в конец полосы пула (порядок пула прежний), запуск в пределах.
            if lanes.wants(p) {
                lanes.push(i, raw)?;
                return lanes.dispatch(p, m);
            }
            // Решение 298: чтение на блоке события опоздало (StaleData) — пул в очередь повтора, сессия идёт дальше;
            // пока ждёт — события без перечитывания.
            let waiting = super::stale_read::waiting(&m.missing, i);
            if listen_log::on_log(p, log, waiting, &g.rx).await? {
                // Ревью t153 (Б1): при 339 брошенный из-за незаданной настройки остаётся брошенным (pool_fail_note.rs).
                let isolate = super::pool_fail::base(&p.ctx).is_some();
                super::pool_fail::requeue(isolate, &mut m.missing, &mut m.abandoned, i);
            }
        }
        return Ok(());
    }
    if log.removed {
        return Ok(());
    }
    match log.block_number {
        Some(block) => admit::wake(p, g, m, i, block).await,
        None => Err(End::Broken(
            "лог подписки без blockNumber — блок пробуждения пула неизвестен".into(),
        )),
    }
}

/// Решение 357 (ревью слияния, Н3): лог глубокого пула пачки (подписка «все события»), не отменённый реоргом. Ошибок нет.
fn deep_log(m: &Members, index: &HashMap<PoolRef, usize>, log: &Log) -> bool {
    if log.removed {
        return false;
    }
    let single = log.topics().get(1).map(|id| PoolRef::Singleton {
        manager: log.address(),
        id: *id,
    });
    single
        .and_then(|r| index.get(&r))
        .or_else(|| index.get(&PoolRef::Contract(log.address())))
        .is_some_and(|&i| m.deep.get(i).copied().unwrap_or(true))
}

/// Добавляет пулы, которых в пачке ещё нет. Ошибок нет.
fn extend(pools: &mut Vec<(Found, String)>, more: Vec<(Found, String)>) {
    for x in more {
        if !pools.iter().any(|(f, _)| f.reference == x.0.reference) {
            pools.push(x);
        }
    }
}

/// Следующая пачка новых пулов; канала нет или он закрыт — ждёт вечно. Ошибок нет.
async fn next_add(adds: &mut Option<Adds>) -> Vec<(Found, String)> {
    if let Some(rx) = adds.as_mut()
        && let Some(v) = rx.recv().await
    {
        return v;
    }
    *adds = None;
    std::future::pending().await
}
