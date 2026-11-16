//! Файл: venues/bsc_dex/pools/per_block_par.rs
//! Что делает: решение 330 — перечитывание пулов «каждым блоком» (per_block.rs on_head) ОДНОВРЕМЕННО, когда задана
//! dex.block_read_parallel_max: свои пулы (WooFi, Lista, Infinity Stable, Nomiswap Stable, Metric, iZiSwap / Biswap
//! V3, KyberSwap DMM, SmarDex — все виды «по одному», load_batch::one_by_one) и пары многомонетных пулов (Curve NG —
//! одно чтение на контракт) — одним чтением группы на блоке головы с пределом одновременных чтений
//! (reload_many::load_group_with → load_batch_par.rs); пулы Balancer V2 (per_block_balancer.rs) и Liquidity Book V2.1
//! (per_block_lb.rs) — своими пачками, одновременно с группой. Раньше — друг за другом, каждый пул со всеми своими
//! обращениями к узлу. Итог тот же: те же функции чтения на том же блоке; не прочитался — пул недостоверен до
//! следующего удачного чтения, при dex.pool_fail_pause_ms — с причиной и паузой (pool_fail.rs, решение 339), как
//! per_block.rs. Сами вызовы вида не объединяются в Multicall3 (это внутренности load_*.rs — зона задачи t152).
//! Кто вызывает: pools/per_block.rs (on_head, on_member_log — failed).
//! От чего зависит: reload_many.rs (load_group_with), per_block_balancer.rs, per_block_lb.rs, pool_fail.rs,
//! listen.rs (Pools, End), discover.rs (Found).
//! Настройки: dex.block_read_parallel_max [bsc_dex] (предел передаёт per_block.rs).

use crate::gateway::errors::GatewayError;

use super::discover::Found;
use super::listen::{End, Pools};
use super::{per_block_balancer, per_block_lb, pool_fail, reload_many};

/// Пулы одного блока, разложенные per_block.rs: свои пулы, пары многомонетных пулов, Balancer V2, LB V2.1.
pub struct Lists<'a> {
    /// Пулы видов «по одному» со своей ссылкой (Contract / Singleton).
    pub singles: Vec<(&'a Found, &'a str)>,
    /// Пары многомонетных пулов (PoolRef::Multi) — группа читает их одним чтением на контракт.
    pub sites: Vec<(&'a Found, &'a str)>,
    /// Пулы Balancer V2 (per_block_balancer.rs).
    pub balancer: &'a [&'a (Found, String)],
    /// Пулы Liquidity Book V2.1 (per_block_lb.rs).
    pub lb: &'a [&'a (Found, String)],
}

/// Что делает: новый блок — все пулы списков одновременно (см. шапку). Принимает: окружение, списки, блок, предел
/// одновременных чтений группы. Ошибки: End — ядро закрыло канал (первая из частей).
pub async fn on_head(p: &Pools, lists: Lists<'_>, block: u64, par: usize) -> Result<(), End> {
    let mut items = lists.singles;
    items.extend(lists.sites);
    let group = async {
        if items.is_empty() {
            return Ok(());
        }
        let res = reload_many::load_group_with(p, &items, block, Some(par)).await?;
        failed(p, &items, &res).await;
        Ok::<(), End>(())
    };
    let (g, b, l) = tokio::join!(
        group,
        per_block_balancer::on_head(p, lists.balancer, block),
        per_block_lb::on_head(p, lists.lb, block)
    );
    g?;
    b?;
    l
}

/// Что делает: не прочитавшиеся пулы группы — недостоверны до следующего удачного чтения, при изоляции — с причиной и
/// паузой (pool_fail::block_failed), как у per_block.rs. Принимает: окружение, пулы группы, итоги (в том же порядке).
/// Ошибок нет.
pub async fn failed(p: &Pools, items: &[(&Found, &str)], res: &[Result<(), GatewayError>]) {
    for ((f, _), r) in items.iter().zip(res) {
        if let Err(why) = r {
            pool_fail::block_failed(p, f, &why.message).await;
        }
    }
}
