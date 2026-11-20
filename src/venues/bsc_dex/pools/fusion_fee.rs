//! Файл: venues/bsc_dex/pools/fusion_fee.rs
//! Что делает: решение 463 (T463-02) — комиссия, которую получит своп пула THENA Fusion (вид thena_fusion) в СЛЕДУЮЩЕМ
//! блоке: данные оракула пула на блоке (fusion_oracle.rs) и расчёт math/fee_algebra_v1.rs. Её и держит
//! PoolState::Algebra.fee вида thena_fusion — котировка и глубина (quote/pool_quote.rs, quote/depth_band.rs) считают с
//! ней. Когда читается: при каждом чтении пула (load_algebra.rs), после каждого Swap пула на блоке события
//! (events_algebra.rs → Applied::Fee → integral_fee.rs apply → read_fee: Swap меняет точку оракула и
//! volumePerLiquidityInBlock) и заранее на голове у края горизонта (fusion_head_read.rs). Опроса нет.
//!
//! Начало окна берётся на время блока + 0 … + 1 + dex.fusion_fee_horizon_s: по нему комиссия пересчитывается на каждой
//! новой голове без обращения к узлу (fusion_head.rs; данные — fusion_book.rs remember). Ревью t236 (пп. 1, 2):
//! настройка обязательна и больше 0 (не задана — NotConfigured с её именем: пул не читается и ждёт её, revive.rs;
//! 0 — так же: при 0 тихий пул перечитывался бы каждым блоком); так же обязательна dex.fusion_oracle_chunk (> 0).
//! Комиссия следующего блока не считается — ошибка с причиной, включая причины отсутствующих частей начала окна
//! (откат getTimepoints).
//! Кто вызывает: pools/load_algebra.rs (next_fee), pools/integral_fee.rs (read_fee — после Swap THENA Fusion),
//! pools/fusion_head_read.rs (read_fee — край горизонта на голове).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_algebra.rs, fusion_oracle.rs, fusion_book.rs (remember),
//! math/fee_algebra_v1.rs.
//! Настройки: dex.fusion_fee_horizon_s [bsc_dex] (обязательна для THENA Fusion; горизонт начала окна, секунды);
//! dex.fusion_oracle_chunk [bsc_dex] (обязательна; точек в одном eth_call getTimepoints).
//!
//! Источник: код пула THENA Fusion — Sourcify 56, пул 0x1b9a1120…ef7d, полное совпадение (AlgebraPool.sol globalState,
//! _getNewFee); оператор — см. fusion_oracle.rs.

use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::fee_algebra_v1;
use super::super::node::call;
use super::abi_algebra::IAlgebraPool as P;
use super::fusion_oracle::{self, bad};

/// Состояние пула на блоке, нужное комиссии: тик, индекс последней точки оракула, globalState.fee.
#[derive(Debug, Clone, Copy)]
pub struct Head {
    pub tick: i32,
    pub index: u16,
    pub fee: u32,
}

/// Настройка горизонта начала окна (решение 463, задача координатора к T463-02).
pub const HORIZON: &str = "dex.fusion_fee_horizon_s";
/// Настройка: точек начала окна в одном eth_call getTimepoints (ревью t236: размер части — не константа кода).
pub const CHUNK: &str = "dex.fusion_oracle_chunk";

/// Комиссия следующего блока: globalState пула на блоке, затем next_fee. Принимает: окружение, пул, блок-тег.
/// Возвращает: комиссию (пипсы). Ошибки: как у next_fee.
pub async fn read_fee(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<u32> {
    let gs = call::view(ctx, pool, &P::globalStateCall {}, tag).await?;
    let head = Head {
        tick: gs.tick.as_i32(),
        index: gs.timepointIndex,
        fee: u32::from(gs.fee),
    };
    next_fee(ctx, pool, head, tag).await
}

/// Комиссия следующего блока по состоянию пула `head` на блоке `tag` (fee_algebra_v1::next_block_fee); данные — в
/// книгу fusion_book.rs (remember). Принимает: окружение, пул, состояние, блок-тег. Возвращает: комиссию (пипсы).
/// Ошибки: NotConfigured (dex.fusion_fee_horizon_s или dex.fusion_oracle_chunk не задана или 0); узел;
/// BadResponse — ответ не по ABI, откат нужного вызова, комиссия не считается (нужного начала окна нет — с причинами
/// частей getTimepoints).
pub async fn next_fee(ctx: &Ctx, pool: Address, head: Head, tag: &str) -> GatewayResult<u32> {
    let h = horizon(ctx)?;
    let chunk = positive(ctx, CHUNK)?;
    let got = fusion_oracle::read(ctx, pool, head, tag, (h, chunk)).await?;
    let Some(fee) = fee_algebra_v1::next_block_fee(&got.oracle, got.ts, head.tick) else {
        let why = format!(
            "комиссия следующего блока (время блока {} и + 1 с) не считается — нет начала окна оракула на это время \
             или параметры feeConfig дают откат{}",
            got.ts,
            gaps(&got.gaps)
        );
        return Err(bad(ctx, pool, &why));
    };
    // Пересчёт на новых головах без узла (fusion_head.rs) — по этим же данным.
    if let Ok(block) = u64::from_str_radix(tag.trim_start_matches("0x"), 16) {
        super::fusion_book::remember(ctx.place.id, pool, block, got);
    }
    Ok(fee)
}

/// Причины отсутствующих частей начала окна для текста ошибки («» — нет). Ошибок нет.
pub fn gaps(g: &[String]) -> String {
    if g.is_empty() {
        return String::new();
    }
    format!("; не прочитано: {}", g.join("; "))
}

/// dex.fusion_fee_horizon_s: на сколько секунд после блока чтения берётся начало окна оракула (пересчёт комиссии на
/// новых головах без узла — fusion_head.rs). Возвращает: секунды (> 0). Ошибки: NotConfigured — не задана, 0 или
/// больше u32::MAX − WINDOW − 1 (secondsAgo getTimepoints — uint32), с честной причиной и именем настройки.
pub fn horizon(ctx: &Ctx) -> GatewayResult<u32> {
    let h = ctx.int(HORIZON)?;
    let max = fee_algebra_v1::HORIZON_MAX;
    match u32::try_from(h) {
        Ok(v) if v > 0 && v <= max => Ok(v),
        _ if h == 0 => Err(not_set(ctx, HORIZON, "нужно целое больше 0")),
        // Ревью t236: secondsAgo = WINDOW + горизонт + 1 идёт в getTimepoints(uint32[] secondsAgos).
        _ => Err(not_set(
            ctx,
            HORIZON,
            &format!(
                "больше {max}: secondsAgo = WINDOW ({}) + горизонт + 1 не помещается в uint32 \
                 (getTimepoints(uint32 time, uint32[] secondsAgos, …) оператора)",
                fee_algebra_v1::WINDOW
            ),
        )),
    }
}

/// Целая настройка больше 0 (dex.fusion_oracle_chunk). Принимает: окружение, имя. Возвращает: значение.
/// Ошибки: NotConfigured — не задана или 0 (имя настройки — в ошибке).
pub fn positive(ctx: &Ctx, name: &str) -> GatewayResult<usize> {
    let n = ctx.int(name)?;
    match usize::try_from(n) {
        Ok(v) if v > 0 => Ok(v),
        _ => Err(not_set(ctx, name, "нужно целое больше 0")),
    }
}

/// NotConfigured «значение не подходит» с именем настройки и причиной. Ошибок нет.
fn not_set(ctx: &Ctx, name: &str, why: &str) -> crate::gateway::errors::GatewayError {
    let v = ctx.int(name).map(|v| v.to_string()).unwrap_or_default();
    let mut e = ctx.err(ErrorCode::NotConfigured, format!("{name} = {v}: {why}"));
    e.setting = Some(name.to_string());
    e
}
