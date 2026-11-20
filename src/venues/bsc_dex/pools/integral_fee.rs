//! Файл: venues/bsc_dex/pools/integral_fee.rs
//! Что делает: решение 325 — у пула Algebra Integral перечитывается ТОЛЬКО комиссия, когда событие меняет только её
//! (Applied::Fee, events_algebra_integral.rs): Fee(uint16) — новая lastFee; SwapFee и Swap 1.2.0/1.2.1 с overrideFee = 0
//! и pluginFee ≠ 0 — цена, ликвидность и тик уже применены из события, неизвестна только комиссия. Раньше в этих
//! случаях пул перечитывался целиком (голова и обход списка тиков: 4–5 + до 2 × dex.integral_tick_steps обращений
//! подряд). Здесь — globalState и fee() одним обращением (node/call.rs try_many: Multicall3.aggregate3 при
//! dex.multicall3, иначе пачка JSON-RPC одним HTTP-запросом) на блоке события; комиссия — по тому же правилу, что у
//! полного чтения (load_algebra_integral.rs: флаг DYNAMIC_FEE в pluginConfig — fee() пула, иначе lastFee; правило —
//! одна функция `dynamic` на оба места).
//! Включаемый: dex.event_refresh_fee_only = да для DEX пула (область «DEX»; ячейка памяти — event_flags.rs). Нет / не
//! задана — Applied::Fee превращается в прежний Refresh (полное перечитывание с той же записью журнала), 1 в 1 как
//! раньше. Ревью 325: «только комиссия» — только у ДОСТОВЕРНОГО пула (valid = true; Н2: недостоверный пул прежняя полная
//! перечитка делает снова достоверным — renewed и «пул готов», listen_log.rs ветка Refresh), только с состоянием
//! Algebra (Н3: иначе комиссию некуда записать — полная перечитка) и не в очереди повтора (решение 298) — fits.
//! Если к записи состояние всё же не Algebra (пул перезаписан другим чтением между проверкой и записью) — ошибка
//! Internal (событие НЕ отмечается учтённым; пул недостоверен, как при любой ошибке чтения, listen_log.rs).
//! Замер (log.pool_refresh_timing) — запись dex.pool_refresh с причиной «fee» (refresh_event.rs fee, Н4).
//! Кто вызывает: pools/refresh_event.rs (apply — через fee), pools/listen_log.rs (fits), pools/load_algebra_integral.rs
//! (dynamic).
//! От чего зависит: abi_algebra_integral.rs, node/call.rs (try_many, decode, block_tag), registry.rs, listen.rs (Pools),
//! event_flags.rs (настройка в памяти).
//! Решение 463 (T463-02): тем же путём после Swap пула THENA Fusion (events_algebra.rs → Applied::Fee) перечитывается
//! комиссия следующего блока (fusion_fee.rs read_fee) — всегда, без dex.event_refresh_fee_only (базовое поведение:
//! комиссия обмена — та, что даст контракт); остальные условия fits — те же.
//! Настройки: dex.event_refresh_fee_only [DEX] (не для THENA Fusion); dex.multicall3 [bsc_dex] (через try_many).
//!
//! ABI — cryptoalgebra/Algebra, integral-core 1.2.10, коммит 1ff43e11407872f7c09b001cdeee1a59d2c334ec:
//! https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/core/contracts/interfaces/pool/IAlgebraPoolState.sol#L44
//! globalState() (… lastFee, pluginConfig …), #L117 fee() (при DYNAMIC_FEE — getCurrentFee() плагина:
//! https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/core/contracts/base/AlgebraPoolBase.sol#L145-L150);
//! DYNAMIC_FEE — https://github.com/cryptoalgebra/Algebra/blob/1ff43e11407872f7c09b001cdeee1a59d2c334ec/src/core/contracts/libraries/Plugins.sol#L26
//! (abi_algebra_integral.rs).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use chrono::Utc;

use crate::core::types::{PoolRef, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::{self, block_tag};
use super::abi_algebra_integral::{DYNAMIC_FEE, IAlgebraIntegralPool as P};
use super::families::Proto;
use super::listen::Pools;
use super::registry::Entry;

/// Комиссию определяет fee() пула (флаг DYNAMIC_FEE в pluginConfig), а не lastFee. Ошибок нет.
pub fn dynamic(plugin_config: u8) -> bool {
    plugin_config & DYNAMIC_FEE != 0
}

/// Идёт ли событие «только комиссия» (Applied::Fee) путём этого файла: dex.event_refresh_fee_only = да для DEX пула,
/// пул не ждёт повтора (решение 298), достоверен (ревью 325, Н2) и его состояние — Algebra (Н3). Иначе — прежний
/// Refresh. Принимает: окружение, запись пула (до события), ждёт ли пул повтора. Ошибок нет.
pub fn fits(ctx: &Ctx, e: &Entry, waiting: bool) -> bool {
    // Решение 463: THENA Fusion — комиссия следующего блока после Swap — базовое поведение, без настройки.
    let allowed = e.family.proto == Proto::Algebra || super::event_flags::fee_only(ctx, e.family);
    !waiting && e.valid && matches!(e.pool.state, PoolState::Algebra { .. }) && allowed
}

/// Комиссия пула на блоке одним обращением. Принимает: окружение, пул, блок-тег.
/// Возвращает: комиссию (пипсы, 1e-6). Ошибки: узел; BadResponse — откат нужного вызова, ответ не по ABI.
pub async fn read(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<u32> {
    let calls = [
        (pool, Bytes::from(P::globalStateCall {}.abi_encode())),
        (pool, Bytes::from(P::feeCall {}.abi_encode())),
    ];
    let got = call::try_many(ctx, &calls, tag).await?;
    let pick = |i: usize| -> GatewayResult<Bytes> {
        match got.get(i) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(why)) => Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Algebra Integral {pool}: {why}"),
            )),
            None => Err(ctx.err(
                ErrorCode::BadResponse,
                format!("Algebra Integral {pool}: ответов меньше, чем вызовов"),
            )),
        }
    };
    let gs = call::decode::<P::globalStateCall>(&pick(0)?)?;
    if dynamic(gs.pluginConfig) {
        return Ok(u32::from(call::decode::<P::feeCall>(&pick(1)?)?));
    }
    Ok(u32::from(gs.lastFee))
}

/// Перечитывает только комиссию пула на блоке события и отмечает событие учтённым (как Applied::Updated в
/// events.rs: блок, позиция лога). Принимает: окружение пачки, ссылку, протокол вида (THENA Fusion — комиссия
/// следующего блока, решение 463), блок и logIndex события.
/// Ошибки: как у read; Internal — ссылка не на свой контракт или состояние пула не Algebra (ревью 325, Н3: событие не
/// отмечается учтённым).
pub async fn apply(
    p: &Pools,
    r: &PoolRef,
    proto: Proto,
    block: u64,
    index: Option<u64>,
) -> GatewayResult<()> {
    let PoolRef::Contract(pool) = r else {
        return Err(p.ctx.err(
            ErrorCode::Internal,
            "Algebra Integral: ссылка на пул не того вида",
        ));
    };
    let tag = block_tag(block);
    // Решение 463: THENA Fusion — комиссия следующего блока по оракулу пула (fusion_fee.rs); Integral — как раньше.
    let fee = if proto == Proto::Algebra {
        super::fusion_fee::read_fee(&p.ctx, *pool, &tag).await?
    } else {
        read(&p.ctx, *pool, &tag).await?
    };
    // Пула нет в реестре (снят) — записывать некуда, как у Updated (listen_log.rs: дальше — ничего).
    match p.reg.update(r, |e| set_fee(e, fee, block, index)) {
        Some(false) => Err(p.ctx.err(
            ErrorCode::Internal,
            format!("Algebra Integral {pool}: состояние пула не Algebra — комиссия не записана"),
        )),
        _ => Ok(()),
    }
}

/// Записывает комиссию в состояние Algebra и отмечает событие учтённым. Состояние не Algebra — запись не меняется.
/// Принимает: запись, комиссию (пипсы), блок и logIndex события. Возвращает: true — записано. Ошибок нет.
pub fn set_fee(e: &mut Entry, fee: u32, block: u64, index: Option<u64>) -> bool {
    let PoolState::Algebra { fee: f, .. } = &mut e.pool.state else {
        return false;
    };
    *f = fee;
    e.pool.block = block;
    e.synced_block = block;
    e.synced_log = index;
    e.pool.received_at = Utc::now();
    true
}
