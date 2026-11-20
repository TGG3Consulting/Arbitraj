//! Файл: venues/bsc_dex/pools/load_ekubo.rs
//! Что делает: чтение состояния пула Ekubo на блоке (решение 447): слот состояния у ядра (Core.sload), у
//! концентрированного пула — слова битмапа тиков по dex.ekubo_bitmap_words по каждую сторону от слова текущего тика и
//! тики всех установленных битов (TickInfo: liquidityNet, liquidityDelta), у пула с MEVCapture — состояние расширения
//! (MEVCapture.sload слота PoolId), время блока. Всё одним блоком (номер в каждом запросе). config пула — из памяти ключей
//! (ekubo_keys.rs, кладёт проверка пула).
//! Кто вызывает: pools/load.rs (ветка Proto::Ekubo).
//! От чего зависит: ctx.rs, node/rpc.rs, abi_ekubo.rs, ekubo_keys.rs, load_smardex.rs (время блока),
//! math/ekubo_config.rs, math/ekubo_bitmap.rs, math/ekubo_tick.rs.
//! Настройки: dex.ekubo_bitmap_words, dex.ekubo_mev_capture [DEX].
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/libraries/CoreStorageLayout.sol L47-L93 (слоты),
//! src/base/ExposedStorage.sol L151-L156 (sload), src/math/tickBitmap.sol L178-L193 (слово и бит тика),
//! src/extensions/MEVCapture.sol L42 (ExposedStorage), L48-L52 (состояние пула — слот PoolId).

use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::json;

use crate::core::types::{EkuboData, EkuboMev, EkuboTick, PoolState};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::ekubo_bitmap::{tick_of, word_index};
use super::super::math::ekubo_config::{self, Shape};
use super::super::math::ekubo_tick::{MAX_TICK, MIN_TICK};
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::abi_ekubo::{bitmap_slot, decode_state, decode_tick, sload_data, sload_words, tick_slot};
use super::families::Family;
use super::{ekubo_keys, load_smardex};

/// Слова битмапа вокруг тика: (первое, последнее), обрезанные до слов MIN_TICK / MAX_TICK. Ошибок нет.
pub fn words_around(tick: i32, spacing: u32, n: u64) -> (u64, u64) {
    let (center, _) = word_index(tick, spacing);
    let (lo_max, _) = word_index(MIN_TICK, spacing);
    let (hi_max, _) = word_index(MAX_TICK, spacing);
    (center.saturating_sub(n).max(lo_max), (center + n).min(hi_max))
}

/// Тики установленных битов слов. Принимает: (слово, биты), шаг. Возвращает: тики по возрастанию. Ошибок нет.
pub fn set_ticks(bitmap: &[(u64, U256)], spacing: u32) -> Vec<i32> {
    let mut out = Vec::new();
    for (w, b) in bitmap {
        for bit in 0..256u32 {
            if b.bit(bit as usize) {
                out.push(tick_of(*w, bit, spacing) as i32);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Значения слотов контракта на блоке (Core.sload / MEVCapture.sload). Ошибки: узел, ответ не той длины.
async fn sload(ctx: &Ctx, at: Address, slots: &[B256], tag: &str) -> GatewayResult<Vec<B256>> {
    if slots.is_empty() {
        return Ok(Vec::new());
    }
    let input = sload_data(slots);
    let raw: Bytes = rpc::call(ctx, "eth_call", json!([{ "to": at, "input": input }, tag])).await?;
    sload_words(&raw, slots.len()).ok_or_else(|| {
        let why = format!(
            "Ekubo {at}: ответ sload — {} байт, ждали {}",
            raw.len(),
            slots.len() * 32
        );
        ctx.err(ErrorCode::BadResponse, why)
    })
}

/// Состояние пула на блоке. Принимает: окружение, вид, ядро, PoolId, блок. Ошибки: Internal (ключа нет в памяти),
/// NotConfigured (dex.ekubo_bitmap_words, dex.ekubo_mev_capture у пула с MEVCapture), узел, BadResponse (пул не
/// создан, расширение пула не то, ответ не той длины).
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    core: Address,
    id: B256,
    block: u64,
) -> GatewayResult<PoolState> {
    let bad = |w: String| ctx.err(ErrorCode::BadResponse, format!("{} {id}: {w}", f.title));
    let Some(config) = ekubo_keys::get(ctx.place.id, id) else {
        let why = format!(
            "{} {id}: ключ пула не известен (пул не прошёл проверку)",
            f.title
        );
        return Err(ctx.err(ErrorCode::Internal, why));
    };
    let tag = block_tag(block);
    let state = sload(ctx, core, &[id], &tag).await?;
    if state[0].is_zero() {
        return Err(bad("пул не создан в ядре".into()));
    }
    let (sqrt_ratio, tick, liquidity) = decode_state(&state[0]);
    let (mut words, mut bitmap, mut ticks) = ((0, 0), Vec::new(), Vec::new());
    if let Shape::Concentrated { spacing } = ekubo_config::shape(&config) {
        let n = ctx.dex_int("dex.ekubo_bitmap_words", f.id)?;
        words = words_around(tick, spacing, n);
        let list: Vec<u64> = (words.0..=words.1).collect();
        let slots: Vec<B256> = list.iter().map(|w| bitmap_slot(id, *w)).collect();
        let got = sload(ctx, core, &slots, &tag).await?;
        for (w, v) in list.iter().zip(got) {
            if !v.is_zero() {
                bitmap.push((*w, U256::from_be_bytes(v.0)));
            }
        }
        let at = set_ticks(&bitmap, spacing);
        let slots: Vec<B256> = at.iter().map(|t| tick_slot(id, *t)).collect();
        let got = sload(ctx, core, &slots, &tag).await?;
        for (t, v) in at.iter().zip(got) {
            let (net, delta) = decode_tick(&v);
            ticks.push(EkuboTick {
                tick: *t,
                net,
                delta,
            });
        }
    }
    let ext = ekubo_config::extension(&config);
    let mev = if ext == Address::ZERO {
        None
    } else {
        let want = ctx.dex_addr("dex.ekubo_mev_capture", f.id)?;
        if want != ext {
            return Err(bad(format!("расширение пула {ext} — не MEVCapture {want}")));
        }
        let m = sload(ctx, ext, &[id], &tag).await?;
        Some(mev_state(&m[0]))
    };
    let block_ts = load_smardex::block_secs(ctx, block).await?;
    let d = EkuboData {
        config,
        sqrt_ratio,
        tick,
        liquidity,
        words,
        bitmap,
        ticks,
        mev,
        block_ts,
    };
    Ok(PoolState::Ekubo(Box::new(d)))
}

/// MEVCapturePoolState (mevCapturePoolState.sol L10-L20): lastUpdateTime — старшие 32 бита, tickLast — младшие 32 со
/// знаком. Ошибок нет.
pub fn mev_state(w: &B256) -> EkuboMev {
    EkuboMev {
        last_update_time: u32::from_be_bytes([w[0], w[1], w[2], w[3]]),
        tick_last: i32::from_be_bytes([w[28], w[29], w[30], w[31]]),
    }
}
