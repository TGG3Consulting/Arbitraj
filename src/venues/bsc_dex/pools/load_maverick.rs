//! Файл: venues/bsc_dex/pools/load_maverick.rs
//! Что делает: чтение состояния пула Maverick V2 (решение 122 F) на блоке — одна пачка Multicall3 частями по
//! dex.load_batch (откат вызова — ответ): getState(), tickSpacing(), fee(true), fee(false), tokenA(), tokenAScale(),
//! tokenBScale() пула и getTicksAroundActive(pool, dex.tick_radius) у PoolLens (непустые тики диапазона активный ±
//! радиус — MaverickV2PoolLens.sol, проверенный исходник 0x942646b0… в эксплорере Arbitrum, L3533-L3567). Собирает
//! PoolState::TickBin — всё, что нужно порту расчёта обмена (math/maverick_swap.rs). tokenA пула обязан быть token0 пары
//! (tokenA < tokenB — IMaverickV2Pool.sol L231-L233): иначе ошибка, а не перестановка.
//! Кто вызывает: pools/load.rs (ветка Proto::Maverick); quote/quoter_maverick.rs и quote/depth_band.rs (radius).
//! От чего зависит: ctx.rs, node/call.rs (try_chunks), load_batch.rs (batch), abi_maverick.rs, core/types (TickBinData).
//! Настройки: dex.pool_lens, dex.tick_radius [DEX], dex.load_batch [bsc_dex].
//!
//! Геттеры — abi_maverick.rs (IMaverickV2Pool.sol L212, L217, L233, L252-L260, L290; IMaverickV2PoolLens.sol L263-L266).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};

use crate::core::types::{PoolState, TickBinData, TickReserves};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_maverick::{IMaverickV2Pool as P, IMaverickV2PoolLens as Lens};
use super::families::Family;

/// Радиус тиков dex.tick_radius как int32 (tickRadius у PoolLens). Ошибки: NotConfigured (не задана или больше int32).
pub fn radius(ctx: &Ctx, f: &Family) -> GatewayResult<i32> {
    let r = ctx.dex_int("dex.tick_radius", f.id)?;
    i32::try_from(r).map_err(|_| ctx.not_set("dex.tick_radius", "dex.tick_radius больше int32"))
}

/// Состояние пула на блоке. Принимает: окружение, вид, адрес пула, пару (token0, token1), блок-тег.
/// Ошибки: настройки (NotConfigured), узел, ответ не по ABI, откат геттера, tokenA пула ≠ token0 пары, нулевой масштаб.
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<PoolState> {
    let lens = ctx.dex_addr("dex.pool_lens", f.id)?;
    let tick_radius = radius(ctx, f)?;
    let calls: Vec<(Address, Bytes)> = vec![
        (pool, P::getStateCall {}.abi_encode().into()),
        (pool, P::tickSpacingCall {}.abi_encode().into()),
        (pool, P::feeCall { tokenAIn: true }.abi_encode().into()),
        (pool, P::feeCall { tokenAIn: false }.abi_encode().into()),
        (pool, P::tokenACall {}.abi_encode().into()),
        (pool, P::tokenAScaleCall {}.abi_encode().into()),
        (pool, P::tokenBScaleCall {}.abi_encode().into()),
        (
            lens,
            Lens::getTicksAroundActiveCall {
                pool,
                tickRadius: tick_radius,
            }
            .abi_encode()
            .into(),
        ),
    ];
    let got = fetch(ctx, &calls, tag).await?;
    let bad = |what: String| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Maverick V2 {pool}: {what}"),
        )
    };
    if got.len() != calls.len() {
        return Err(bad(format!("ответов {} из {}", got.len(), calls.len())));
    }
    let names = [
        "getState",
        "tickSpacing",
        "fee(true)",
        "fee(false)",
        "tokenA",
        "tokenAScale",
        "tokenBScale",
        "getTicksAroundActive",
    ];
    let mut raw: Vec<&Bytes> = Vec::with_capacity(names.len());
    for (r, name) in got.iter().zip(names) {
        raw.push(
            r.as_ref()
                .map_err(|why| bad(format!("{name} откатился: {why}")))?,
        );
    }
    let st = call::decode::<P::getStateCall>(raw[0])?;
    let tick_spacing = call::decode::<P::tickSpacingCall>(raw[1])?;
    let fee0_in = call::decode::<P::feeCall>(raw[2])?;
    let fee1_in = call::decode::<P::feeCall>(raw[3])?;
    let token_a = call::decode::<P::tokenACall>(raw[4])?;
    if token_a != pair.0 {
        return Err(bad(format!("tokenA {token_a} ≠ token0 пары {}", pair.0)));
    }
    let scale0 = call::decode::<P::tokenAScaleCall>(raw[5])?;
    let scale1 = call::decode::<P::tokenBScaleCall>(raw[6])?;
    if scale0.is_zero() || scale1.is_zero() {
        return Err(bad("нулевой tokenAScale/tokenBScale".into()));
    }
    let around = call::decode::<Lens::getTicksAroundActiveCall>(raw[7])?;
    let ticks = ticks_of(&around.ticks, &around.tickStates)
        .ok_or_else(|| bad("тиков и состояний тиков разное число".into()))?;
    let data = TickBinData {
        active_tick: st.activeTick,
        reserve0: U256::from(st.reserveA),
        reserve1: U256::from(st.reserveB),
        protocol_fee_d3: st.protocolFeeRatioD3,
        tick_spacing,
        fee0_in,
        fee1_in,
        scale0,
        scale1,
        tick_lo: st.activeTick.saturating_sub(tick_radius),
        tick_hi: st.activeTick.saturating_add(tick_radius),
        ticks,
    };
    Ok(PoolState::TickBin(Box::new(data)))
}

/// Пачка вызовов на блоке частями по dex.load_batch (Multicall3; node/call.rs try_chunks). Возвращает: по вызову
/// байты или текст отката. Ошибки: NotConfigured (dex.load_batch), узел (сбой любой части — пул не прочитан).
pub async fn fetch(
    ctx: &Ctx,
    calls: &[(Address, Bytes)],
    tag: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let size = super::load_batch::batch(ctx)?;
    call::try_chunks(ctx, calls, tag, size)
        .await
        .into_iter()
        .map(|p| p.map_err(|node| ctx.err(ErrorCode::Unreachable, format!("узел: {node}"))))
        .collect()
}

/// Тики с резервами по возрастанию тика (без сети). None — длины массивов не совпали. Ошибок нет.
pub fn ticks_of(ticks: &[i32], states: &[P::TickState]) -> Option<Vec<TickReserves>> {
    if ticks.len() != states.len() {
        return None;
    }
    let mut out: Vec<TickReserves> = ticks
        .iter()
        .zip(states)
        .map(|(t, s)| TickReserves {
            tick: *t,
            reserve0: s.reserveA,
            reserve1: s.reserveB,
            total_supply: s.totalSupply,
        })
        .collect();
    out.sort_by_key(|t| t.tick);
    Some(out)
}
