//! Файл: venues/bsc_dex/pools/settle_read.rs
//! Что делает: решение 357 — резервы бинов события И активный бин пула на блоке события ОДНИМ запросом к узлу
//! (node/call.rs try_many: Multicall3 aggregate3 при dex.multicall3 — один eth_call; иначе одна пачка JSON-RPC):
//! • Liquidity Book V2.1 (PoolRef::Contract): getBin(id) каждого бина + getActiveId() пары;
//! • PancakeSwap Infinity Bin (PoolRef::Singleton): getBin(PoolId, id) каждого бина + getSlot0(PoolId) менеджера.
//! Активный бин конца блока нужен settle_bins.rs: равен активному бину памяти — логи блока пула применены, пул уходит
//! ядру; не равен — логи ещё идут, пул «в обновлении». Раньше бины события читались тем же try_many (LB, ревью 290) или
//! по одному getBin (Infinity Bin) — активный бин добавлен в тот же запрос, отдельного обращения нет.
//! Решение 445: у пула Liquidity Book v2.0 активный бин — getReservesAndId() (activeId — третье слово ответа; getBin —
//! тот же селектор, ответ того же вида) — calls_for / decode_for / read_for с признаком v20 (у записи LbBinData.v20).
//! Кто вызывает: pools/settle_bins.rs; тесты (tests/bsc_dex_settle_357.rs).
//! От чего зависит: node/call.rs (try_many, decode), abi_lb.rs, abi_lb20.rs, abi_v4.rs, ctx.rs.
//! Настройки: через node/call.rs — dex.multicall3 [bsc_dex].
//!
//! LBPair v2.1.0: getActiveId L171-L173, getBin L181-L183 (lfj-gg/joe-v2, tests/data/lb_v21_2026-11-15/source_excerpt.txt);
//! LBPair v2.0.0 (07f10212): getReservesAndId L147-L158, getBin L265-L267.
//! IBinPoolManager getSlot0 / getBin — IBinPoolManager.sol#L137-L167 (pancakeswap/infinity-core 3c5ef1be, abi_v4.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};

use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_lb::ILBPair as Lb;
use super::abi_lb20::ILBPairV20 as Lb20;
use super::abi_v4::IInfinityBin as Bin;

/// Прочитанное: резервы бинов (id, X, Y) и активный бин конца блока.
pub type Read = (Vec<(u32, u128, u128)>, u32);

/// Вызовы пачки: getBin каждого бина, ПОСЛЕДНИМ — активный бин. Принимает: ссылку пула, бины. Возвращает: вызовы;
/// None — ссылка не своего контракта и не Singleton (бинов у узла не читаем). Ошибок нет.
pub fn calls(r: &PoolRef, ids: &[u32]) -> Option<Vec<(Address, Bytes)>> {
    calls_for(r, ids, false)
}

/// То же с признаком пула LB v2.0 (решение 445): v20 — активный бин getReservesAndId(). Ошибок нет.
pub fn calls_for(r: &PoolRef, ids: &[u32], v20: bool) -> Option<Vec<(Address, Bytes)>> {
    let mut out = Vec::with_capacity(ids.len() + 1);
    match r {
        PoolRef::Contract(pool) => {
            for id in ids {
                let c = Lb::getBinCall { id: U24::from(*id) };
                out.push((*pool, Bytes::from(c.abi_encode())));
            }
            let active = if v20 {
                Lb20::getReservesAndIdCall {}.abi_encode()
            } else {
                Lb::getActiveIdCall {}.abi_encode()
            };
            out.push((*pool, Bytes::from(active)));
        }
        PoolRef::Singleton { manager, id } => {
            for b in ids {
                let c = Bin::getBinCall {
                    id: *id,
                    binId: U24::from(*b),
                };
                out.push((*manager, Bytes::from(c.abi_encode())));
            }
            out.push((
                *manager,
                Bytes::from(Bin::getSlot0Call { id: *id }.abi_encode()),
            ));
        }
        _ => return None,
    }
    Some(out)
}

/// Разбор ответов calls (без сети). Принимает: окружение (текст ошибки), ссылку, бины, ответы по порядку.
/// Возвращает: Read. Ошибки: BadResponse — ответов не столько, сколько вызовов, вызов откатился, ответ не по ABI.
pub fn decode(
    ctx: &Ctx,
    r: &PoolRef,
    ids: &[u32],
    got: Vec<Result<Bytes, String>>,
) -> GatewayResult<Read> {
    decode_for(ctx, r, ids, got, false)
}

/// То же с признаком пула LB v2.0 (решение 445). Ошибки: как decode; активный бин v2.0 больше u32.
pub fn decode_for(
    ctx: &Ctx,
    r: &PoolRef,
    ids: &[u32],
    got: Vec<Result<Bytes, String>>,
    v20: bool,
) -> GatewayResult<Read> {
    if got.len() != ids.len() + 1 {
        let m = format!("{r:?}: ответов {} из {}", got.len(), ids.len() + 1);
        return Err(ctx.err(ErrorCode::BadResponse, m));
    }
    let mut bins = Vec::with_capacity(ids.len());
    let mut it = got.into_iter();
    for id in ids {
        let b = it.next().unwrap_or_else(|| Err("нет ответа".into()));
        let b = b.map_err(|w| {
            ctx.err(
                ErrorCode::BadResponse,
                format!("{r:?}: getBin({id}) откатился: {w}"),
            )
        })?;
        let (x, y) = match r {
            PoolRef::Contract(_) => {
                let v = call::decode::<Lb::getBinCall>(&b)?;
                (v.binReserveX, v.binReserveY)
            }
            _ => {
                let v = call::decode::<Bin::getBinCall>(&b)?;
                (v.binReserveX, v.binReserveY)
            }
        };
        bins.push((*id, x, y));
    }
    let a = it.next().unwrap_or_else(|| Err("нет ответа".into()));
    let a = a.map_err(|w| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("{r:?}: активный бин откатился: {w}"),
        )
    })?;
    let active: u32 = match r {
        PoolRef::Contract(_) if v20 => {
            let id = call::decode::<Lb20::getReservesAndIdCall>(&a)?.activeId;
            u32::try_from(id).map_err(|_| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("{r:?}: активный бин {id} вне u32"),
                )
            })?
        }
        PoolRef::Contract(_) => call::decode::<Lb::getActiveIdCall>(&a)?.to(),
        _ => call::decode::<Bin::getSlot0Call>(&a)?.activeId.to(),
    };
    Ok((bins, active))
}

/// Бины и активный бин на блоке `tag` одним запросом. Возвращает: Some(Read); None — ссылка не читается у узла.
/// Ошибки: узел (в т. ч. StaleData — решение 298), BadResponse (decode).
pub async fn read(ctx: &Ctx, r: &PoolRef, ids: &[u32], tag: &str) -> GatewayResult<Option<Read>> {
    read_for(ctx, r, ids, tag, false).await
}

/// То же с признаком пула LB v2.0 (решение 445). Ошибки: как read.
pub async fn read_for(
    ctx: &Ctx,
    r: &PoolRef,
    ids: &[u32],
    tag: &str,
    v20: bool,
) -> GatewayResult<Option<Read>> {
    let Some(c) = calls_for(r, ids, v20) else {
        return Ok(None);
    };
    let got = call::try_many(ctx, &c, tag).await?;
    decode_for(ctx, r, ids, got, v20).map(Some)
}
