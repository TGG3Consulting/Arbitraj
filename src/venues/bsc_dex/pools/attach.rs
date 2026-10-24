//! Файл: venues/bsc_dex/pools/attach.rs
//! Что делает: подключает к уже идущей подписке на рынок пулы, найденные после её старта (новые пулы в сети,
//! пулы V4 из фоновой загрузки истории): только тех, у которых ещё нет слушателя, пачками по dex.pools_per_sub —
//! без перезапуска остальных слушателей.
//! Кто вызывает: pools/created.rs (новые пулы), pools/history_v4.rs (пулы V4 по мере загрузки истории).
//! От чего зависит: listen.rs, market.rs (пачки и фильтры подписки), created.rs (Watch), node/subs.rs.
//! Настройки: dex.pools_per_sub [bsc_dex].

use std::collections::HashMap;

use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::node::subs;
use super::created::Watch;
use super::discover::Found;
use super::listen::{self, Pools};
use super::market;

/// Запускает слушателей для пулов, которых ещё нет.
/// Принимает: окружение, наблюдатель (монеты, встречный токен, известные пулы, задачи), найденные пулы,
/// each — писать ли в журнал каждый пул (новые пулы сети) или одну строку на пачку (история V4).
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
    if each {
        for f in &fresh {
            let ticker = w.coins.get(&f.coin).cloned().unwrap_or_default();
            let msg = format!("Новый пул {} {ticker}: {:?}", f.family.title, f.reference);
            p.ctx.log(Level::Info, "dex.pool", msg, None).await;
        }
    } else {
        let mut by: HashMap<&str, usize> = HashMap::new();
        for f in &fresh {
            *by.entry(f.family.title).or_default() += 1;
        }
        let msg = format!("Подключены пулы из истории V4: {by:?}");
        p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    }
    let n = fresh.len();
    for (name, chunk, filter) in market::chunks(&fresh, &w.coins, per) {
        let h = tokio::spawn(listen::run(p.clone(), format!("{name}-new"), chunk, filter));
        w.tasks.lock().unwrap_or_else(|x| x.into_inner()).push(h);
    }
    Ok(n)
}
