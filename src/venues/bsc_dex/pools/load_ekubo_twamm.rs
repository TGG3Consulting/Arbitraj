//! Файл: venues/bsc_dex/pools/load_ekubo_twamm.rs
//! Что делает: чтение состояния расширения TWAMM пула Ekubo на блоке (решение 454): TwammPoolState (время последнего
//! исполнения виртуальных ордеров, скорости продажи token0 / token1), слова битмапа времён всех допустимых времён от
//! последнего исполнения до «время блока + dex.ekubo_twamm_horizon_s» и TimeInfo (дельты скоростей) установленных
//! битов. До этого времени включительно котировка может провести виртуальные ордера (math/ekubo_virtual.rs; слово
//! первого допустимого времени за горизонтом на результат не влияет — math/ekubo_time.rs search); позже — пул нужно
//! перечитать (котировка отвечает причиной). Всё — TWAMM.sload на том же блоке.
//! Кто вызывает: pools/load_ekubo.rs.
//! От чего зависит: ctx.rs, load_ekubo.rs (sload), math/ekubo_time.rs, math/ekubo_virtual.rs (real_last).
//! Настройки: dex.ekubo_twamm_horizon_s [DEX].
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/libraries/TWAMMStorageLayout.sol L20-L24 (смещения), L34-L36
//! (состояние — слот PoolId), L50-L54 (битмап времён — PoolId + TIME_BITMAPS + слово), L60-L64 (TimeInfo —
//! PoolId + TIME_INFOS + время); src/types/twammPoolState.sol L14-L44 (время — младшие 32 бита, скорость
//! token0 — биты 32..143, token1 — 144..255); src/types/timeInfo.sol L5-L37 (дельты — int112 в битах 112..223 и
//! 0..111); src/extensions/TWAMM.sol L12, L60 (ExposedStorage: sload()).

use alloy_primitives::{Address, B256, U256};

use crate::core::types::EkuboTwamm;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::math::ekubo_time::words_until;
use super::super::math::ekubo_virtual::real_last;
use super::families::Family;
use super::load_ekubo::sload;

/// Смещения TWAMMStorageLayout TIME_BITMAPS_OFFSET / TIME_INFOS_OFFSET (L22, L24) — hex без 0x.
const TIME_BITMAPS: &str = "07f3f693b68a1a1b1b3315d4b74217931d60e9dc7f1af4989f50e7ab31c8820e";
const TIME_INFOS: &str = "70db18ef1c685b7aa06d1ac5ea2d101c7261974df22a15951f768f92187043fb";

/// Слот PoolId + смещение + n (по модулю 2^256). Ошибок нет.
fn slot(id: B256, offset: &str, n: u64) -> B256 {
    let off = U256::from_str_radix(offset, 16).unwrap_or_default();
    let v = U256::from_be_bytes(id.0)
        .wrapping_add(off)
        .wrapping_add(U256::from(n));
    B256::from(v.to_be_bytes::<32>())
}

/// int112 из 112 бит слова со сдвигом `shift` (signextend(13, …)). Ошибок нет.
fn int112(v: U256, shift: usize) -> i128 {
    let mask = (U256::from(1u8) << 112usize) - U256::from(1u8);
    let raw = ((v >> shift) & mask).to::<u128>();
    ((raw << 16) as i128) >> 16
}

/// TwammPoolState: (время, скорость token0, скорость token1). Ошибок нет.
pub fn decode_state(w: &B256) -> (u32, u128, u128) {
    let v = U256::from_be_bytes(w.0);
    let mask = (U256::from(1u8) << 112usize) - U256::from(1u8);
    let last = (v & U256::from(u32::MAX)).to::<u32>();
    let r0 = ((v >> 32usize) & mask).to::<u128>();
    let r1 = (v >> 144usize).to::<u128>();
    (last, r0, r1)
}

/// TimeInfo: (дельта token0, дельта token1). Ошибок нет.
pub fn decode_time(w: &B256) -> (i128, i128) {
    let v = U256::from_be_bytes(w.0);
    (int112(v, 112), int112(v, 0))
}

/// Состояние TWAMM пула на блоке. Принимает: окружение, вид, адрес TWAMM, PoolId, время блока, блок (тег).
/// Ошибки: NotConfigured (dex.ekubo_twamm_horizon_s), узел, BadResponse (ответ sload не той длины).
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    twamm: Address,
    id: B256,
    block_ts: u64,
    tag: &str,
) -> GatewayResult<EkuboTwamm> {
    let horizon = ctx.dex_int("dex.ekubo_twamm_horizon_s", f.id)?;
    let state = sload(ctx, twamm, &[id], tag).await?;
    let (last, rate0, rate1) = decode_state(&state[0]);
    let real = real_last(block_ts, last);
    let until = block_ts.saturating_add(horizon);
    let words = words_until(real, until);
    let slots: Vec<B256> = words.iter().map(|w| slot(id, TIME_BITMAPS, *w)).collect();
    let got = sload(ctx, twamm, &slots, tag).await?;
    let mut bitmap = Vec::new();
    let mut at = Vec::new();
    for (w, v) in words.iter().zip(got) {
        if v.is_zero() {
            continue;
        }
        let bits = U256::from_be_bytes(v.0);
        bitmap.push((*w, bits));
        for i in 0..256usize {
            let t = (*w << 16) + ((i as u64) << 8);
            if bits.bit(i) && t > real {
                at.push(t);
            }
        }
    }
    let slots: Vec<B256> = at.iter().map(|t| slot(id, TIME_INFOS, *t)).collect();
    let got = sload(ctx, twamm, &slots, tag).await?;
    let times = at
        .iter()
        .zip(got)
        .map(|(t, v)| {
            let (d0, d1) = decode_time(&v);
            (*t, d0, d1)
        })
        .collect();
    Ok(EkuboTwamm {
        last,
        rate0,
        rate1,
        words,
        bitmap,
        times,
        until,
    })
}
