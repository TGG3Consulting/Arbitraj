//! Файл: venues/bsc_dex/pools/solidly_refresh.rs
//! Что делает: решение 273 — перечитывание комиссии и ограничений обмена (пауза фабрики, предел цены газа) пулов
//! Solidly, у DEX которых задан источник комиссии (dex.solidly_fee_source) или методы ограничений. Эти значения
//! меняются без события пула (Sync их не несёт) — перечитываются тем же путём, что своя комиссия пар V2
//! (pools/index_watch.rs refresh_fees): перед каждым проходом сверки с квотером (check/quoter_cycle.rs), в реестр.
//! У DEX без этих настроек (THENA V1, Topaz и др.) ничего не перечитывается — как до решения 273.
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: pools/{listen, registry, load_solidly, solidly_fee_read}.rs, core/types (PoolState, PoolRef).
//! Настройки: dex.factory, dex.solidly_fee_source, dex.solidly_pause_method, dex.solidly_max_gas_method [DEX]; через
//! solidly_fee_read.rs — остальные dex.solidly_fee_* [DEX].

use crate::core::types::{PoolRef, PoolState};
use crate::storage::journal::Level;

use super::listen::Pools;
use super::load_solidly;
use super::solidly_fee_read;

/// Перечитывает комиссию и ограничения обмена пулов Solidly (см. шапку) на последнем блоке узла. Ошибок нет: сбой —
/// в журнал, состояние пула прежнее (сверка с квотером на блоке всё равно читает пул заново, check/quoter_block.rs).
pub async fn refresh(p: &Pools) {
    for r in p.reg.refs() {
        let Some(e) = p.reg.get(&r) else { continue };
        let (PoolState::Solidly { stable, .. }, PoolRef::Contract(pool)) = (&e.pool.state, &r) else {
            continue;
        };
        let f = e.family;
        let has_source = matches!(solidly_fee_read::source(&p.ctx, f), Ok(Some(_)));
        if !has_source && !solidly_fee_read::has_gate(&p.ctx, f) {
            continue;
        }
        let read = async {
            let factory = p.ctx.dex_addr("dex.factory", f.id)?;
            let fee = load_solidly::pool_fee(&p.ctx, f, (factory, *pool, *stable), "latest").await?;
            let gate = solidly_fee_read::gate(&p.ctx, f, factory, "latest").await?;
            Ok::<_, crate::gateway::errors::GatewayError>((fee, gate))
        };
        match read.await {
            Ok((nf, ng)) => {
                p.reg.update(&r, |x| {
                    if let PoolState::Solidly { fee, gate, .. } = &mut x.pool.state {
                        *fee = nf;
                        *gate = ng;
                    }
                });
            }
            Err(err) => {
                let msg = format!("{} {pool:#x}: комиссия/ограничения пула не перечитаны: {}", f.id, err.message);
                p.ctx.log(Level::Warn, "dex.index", msg, None).await;
            }
        }
    }
}
