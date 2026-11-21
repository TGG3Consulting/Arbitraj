//! Файл: venues/bsc_dex/pools/fee_refresh.rs
//! Что делает: решение 281 (ревью, В2) — перечтение fee() пулов V3-форка с менеджером комиссии (SquadSwap V3) в реестр:
//! одного пула по событию менеджера и ВСЕХ пулов DEX пачкой (node/call.rs try_chunks, частями по dex.load_batch) —
//! в начале каждой сессии подписки fee_watch.rs (событие SetCustomFee во время обрыва WebSocket иначе теряется) и по
//! UpdatePoolManager фабрики. В журнал (Info) — каждое событие менеджера и каждая изменившаяся при перечтении всех комиссия.
//! Кто вызывает: pools/fee_watch.rs.
//! От чего зависит: listen.rs (Pools), registry.rs, node/call.rs, fee_fresh.rs (read), abi_check.rs (fee()),
//! load_batch.rs (batch — размер части).
//! Настройки: dex.load_batch [bsc_dex] (через load_batch::batch).
//!
//! fee() пула — SquadV3Pool.sol L136-L137 (Sourcify, 0x9fb9815ca1417b06466aaa86521a28ba83d0bd33; ссылки — fee_fresh.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::node::call;
use super::abi_check::IPoolImmutables as P;
use super::families::Family;
use super::fee_fresh;
use super::listen::Pools;
use super::load_batch;

/// Записывает комиссию пула в реестр; запись Info в журнал — если изменилась или always (событие менеджера, как
/// раньше). Принимает: пул, комиссия, откуда (для журнала), always. Ошибок нет.
async fn store(p: &Pools, pool: Address, fee: u32, why: &str, always: bool) {
    let r = PoolRef::Contract(pool);
    let old = p.reg.update(&r, |e| e.pool.fee_pips.replace(fee)).flatten();
    if always || old != Some(fee) {
        let msg = format!("Комиссия пула {pool:#x}: {old:?} → {fee} из 1e6 ({why}, решение 281)");
        p.ctx.log(Level::Info, "dex.pool", msg, None).await;
    }
}

/// Перечитывает fee() пула на узле и пишет в реестр, если пул в нём есть. Ошибок нет: сбой узла — в журнал (Warn).
pub async fn one(p: &Pools, r: PoolRef) {
    let PoolRef::Contract(pool) = r else { return };
    if !p.reg.contains(&r) {
        return;
    }
    match fee_fresh::read(&p.ctx, pool, "latest").await {
        Ok(fee) => store(p, pool, fee, "событие менеджера", true).await,
        Err(e) => {
            let msg = format!("Комиссия пула {pool:#x} не перечитана: {}", e.message);
            p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
        }
    }
}

/// Перечитывает fee() всех пулов DEX из реестра одной пачкой. Принимает: вид DEX, откуда (для журнала).
/// Ошибок нет: не задан dex.load_batch, сбой узла или ответ не по ABI — запись Warn (пулы, что не перечитаны).
pub async fn all(p: &Pools, f: &Family, why: &str) {
    let pools: Vec<Address> = p
        .reg
        .all()
        .into_iter()
        .filter(|e| e.family.id == f.id)
        .filter_map(|e| match e.pool.reference {
            PoolRef::Contract(a) => Some(a),
            _ => None,
        })
        .collect();
    if pools.is_empty() {
        return;
    }
    let size = match load_batch::batch(&p.ctx) {
        Ok(n) => n,
        Err(e) => return warn(p, f, pools.len(), &e.message).await,
    };
    let data = Bytes::from(P::feeCall {}.abi_encode());
    let calls: Vec<(Address, Bytes)> = pools.iter().map(|a| (*a, data.clone())).collect();
    let got = call::try_chunks(&p.ctx, &calls, "latest", size).await;
    let mut missed = Vec::new();
    for (pool, piece) in pools.iter().zip(got) {
        let fee = match piece {
            Ok(Ok(b)) => call::decode::<P::feeCall>(&b)
                .map(|x| x.to::<u32>())
                .map_err(|e| e.message),
            Ok(Err(rev)) => Err(format!("откат: {rev}")),
            Err(node) => Err(node),
        };
        match fee {
            Ok(fee) => store(p, *pool, fee, why, false).await,
            Err(e) => missed.push(format!("{pool:#x}: {e}")),
        }
    }
    if !missed.is_empty() {
        warn(p, f, missed.len(), &missed.join("; ")).await;
    }
}

/// Запись Warn «fee() пулов DEX не перечитаны». Ошибок нет.
async fn warn(p: &Pools, f: &Family, n: usize, why: &str) {
    let msg = format!(
        "{}: fee() {n} пулов не перечитана (решение 281): {why}",
        f.title
    );
    p.ctx.log(Level::Warn, "dex.pool", msg, None).await;
}
