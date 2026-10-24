//! Файл: venues/bsc_dex/pools/attach.rs
//! Что делает: подключает к уже идущей подписке на рынок пулы, найденные после её старта (новые пулы в сети,
//! пулы V4 из фоновой загрузки истории): только тех, у которых ещё нет слушателя. Пулы одного вида идут в открытую
//! пачку этого вида (её соединение и сессия уже работают — session.rs принимает их без нового соединения), пока в ней
//! меньше dex.pools_per_sub пулов; дальше — новая пачка со своим соединением. Раньше каждое подключение открывало
//! своё соединение и подписку: по живому журналу 2026-10-24 это ≈7,9 тыс. ответов на eth_subscribe и
//! eth_blockNumber в час и тысячи соединений.
//! Кто вызывает: pools/created.rs (новые пулы), pools/history_v4.rs (пулы V4 по мере загрузки истории).
//! От чего зависит: listen.rs, session.rs (канал пачки), market.rs (виды), created.rs (Watch), node/subs.rs.
//! Настройки: dex.pools_per_sub [bsc_dex].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::node::subs;
use super::batches;
use super::created::Watch;
use super::discover::Found;
use super::listen::{self, Pools};

/// Открытая пачка вида: канал её сессии, сколько в ней пулов, номер пачки вида.
pub struct Open {
    tx: mpsc::UnboundedSender<Vec<(Found, String)>>,
    count: usize,
    seq: usize,
}

/// Открытые пачки по ключу вида (batches::by_family). Дешёво клонируется.
pub type Late = Arc<Mutex<HashMap<String, Open>>>;

/// Запускает слушание пулов, которых ещё нет.
/// Принимает: окружение, наблюдатель (монеты, встречный токен, известные пулы, задачи, пачки), найденные пулы,
/// each — писать ли в журнал каждый пул (новые пулы сети); история V4 — без записей (итог — в ходе чтения).
/// Возвращает: сколько пулов подключено. Ошибки: NotConfigured (dex.pools_per_sub).
pub async fn attach(p: &Pools, w: &Watch, found: Vec<Found>, each: bool) -> GatewayResult<usize> {
    let per = subs::per_sub(&p.ctx)?;
    let fresh: Vec<Found> = {
        let mut known = w.known.lock().unwrap_or_else(|x| x.into_inner());
        found
            .into_iter()
            // Пулы ищутся только по монетам подписки — монета без тикера значит, что подписка сменилась.
            .filter(|f| w.coins.contains_key(&f.coin))
            .filter(|f| known.insert(f.reference.clone()))
            .collect()
    };
    if fresh.is_empty() {
        return Ok(0);
    }
    // История V4 (each = false) — без записи на кусок: число подключённых — в строке хода чтения (scan.rs Progress).
    if each {
        for f in &fresh {
            let ticker = w.coins.get(&f.coin).cloned().unwrap_or_default();
            let msg = format!("Новый пул {} {ticker}: {:?}", f.family.title, f.reference);
            p.ctx.log(Level::Info, "dex.pool", msg, None).await;
        }
    }
    let n = fresh.len();
    p.loading.add(&fresh);
    p.loading.publish(p).await;
    for (key, family, list) in batches::by_family(&fresh, &w.coins) {
        place(p, w, &key, family, list, per);
    }
    Ok(n)
}

/// Раскладка `n` новых пулов вида: сколько уходит в открытую пачку (в ней `open` пулов; None — открытой нет) и размеры
/// новых пачек, каждая не больше `per`. Ошибок нет.
pub fn split(open: Option<usize>, n: usize, per: usize) -> (usize, Vec<usize>) {
    let into_open = open.map(|c| per.saturating_sub(c).min(n)).unwrap_or(0);
    let mut rest = n - into_open;
    let mut new = Vec::new();
    while rest > 0 && per > 0 {
        let take = rest.min(per);
        new.push(take);
        rest -= take;
    }
    (into_open, new)
}

/// Раскладывает пулы одного вида по split: в открытую пачку, остальное — новые пачки. Ошибок нет.
fn place(p: &Pools, w: &Watch, key: &str, family: &str, list: Vec<(Found, String)>, per: usize) {
    let mut late = w.late.lock().unwrap_or_else(|x| x.into_inner());
    let mut rest = list;
    let open = late.get(key).filter(|o| !o.tx.is_closed()).map(|o| o.count);
    let (into_open, new) = split(open, rest.len(), per);
    if into_open > 0
        && let Some(o) = late.get_mut(key)
    {
        let part: Vec<_> = rest.drain(..into_open).collect();
        o.count += part.len();
        // Сессия пачки кончилась между проверкой и отправкой — пулы уходят в новую пачку ниже.
        if let Err(back) = o.tx.send(part) {
            rest.splice(0..0, back.0);
        }
    }
    let first = late.get(key).map(|o| o.seq + 1).unwrap_or(0);
    let sizes = if rest.len() == new.iter().sum::<usize>() {
        new
    } else {
        split(None, rest.len(), per).1
    };
    for (seq, size) in (first..).zip(sizes) {
        let part: Vec<_> = rest.drain(..size).collect();
        let (tx, rx) = mpsc::unbounded_channel();
        let name = format!("{family}-new-{seq}");
        let h = tokio::spawn(listen::run(p.clone(), name, part.clone(), false, Some(rx)));
        w.tasks.lock().unwrap_or_else(|x| x.into_inner()).push(h);
        late.insert(
            key.to_string(),
            Open {
                tx,
                count: part.len(),
                seq,
            },
        );
    }
}
