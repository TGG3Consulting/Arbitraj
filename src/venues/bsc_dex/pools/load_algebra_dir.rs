//! Файл: venues/bsc_dex/pools/load_algebra_dir.rs
//! Что делает: решение 444 — чтение состояния пула Algebra V1.9 с направленными комиссиями (Camelot V3, вид
//! algebra_dir_fee) на заданном блоке: globalState (цена, тик, комиссии feeZto и feeOtz), liquidity и карта СЫРЫХ тиков
//! (тот же TickTable и ticks(), что у THENA Fusion — load_algebra.rs tick_map). Собирает PoolState::Algebra с
//! fee = feeZto и fee_otz = Some(feeOtz) — math/swap_algebra берёт комиссию стороны (math/algebra_fee.rs).
//! Кто вызывает: pools/load.rs (и через него load_batch.rs — пулы этого вида читаются по одному, как Algebra V1.9).
//! От чего зависит: ctx.rs, node/call.rs, pools/abi_algebra_dir.rs, pools/abi_algebra.rs (liquidity),
//! pools/load_algebra.rs (tick_map), core/types (PoolState).
//! Настройки: через load_algebra.rs tick_map — dex.tick_words, dex.tick_read_band_pct, dex.tick_read_band_algebra_pct
//! [площадка DEX].
//!
//! Источник — cryptoalgebra/AlgebraV1.9, ветка direction-based-fee, коммит a45417ed5048c0a9ad3c673e41a00bb3c3880f6f:
//! globalState — src/core/contracts/interfaces/pool/IAlgebraPoolState.sol:22-33 (PoolState.sol:8-17); liquidity —
//! PoolState.sol:27; комиссия обмена по направлению — AlgebraPool.sol:739. Сырой битмап (rowNumber = tick >> 8, без
//! деления на tickSpacing) — src/core/contracts/libraries/TickTable.sol этой ветки (в коммите a3dc950f до 237524b
//! битмап был сжат на TICK_SPACING = 60; какая версия у пулов фабрики — проверяет запись: CREATE2 адреса пула с хешем
//! кода POOL_INIT_CODE_HASH версии, tools/arbitrum_record/algebra_dir_fee.py).
//!
//! Комиссия — значение globalState на блоке чтения. Первый обмен в новой секунде пишет точку оракула и пересчитывает
//! комиссии до шага цены (AlgebraPool.sol:777-795, _updateFee :549-558) — этого пересчёта здесь нет (как у THENA
//! Fusion, load_algebra.rs); расхождение ловит сверка с квотером (решение 73, виртуальный квотер — решение 186).

use alloy_primitives::{Address, U256};

use crate::core::types::PoolState;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call;
use super::abi_algebra::IAlgebraPool;
use super::abi_algebra_dir::IAlgebraDirPool;
use super::load_algebra::tick_map;
use super::ticks::TickMap;

/// Состояние и карта тиков пула Algebra V1.9 с направленными комиссиями на блоке.
/// Принимает: окружение, адрес пула, блок-тег. Возвращает: (PoolState::Algebra с двумя комиссиями, карта сырых тиков).
/// Ошибки: NotConfigured (dex.tick_words), узел, ответ не по ABI, тик вне int24.
pub async fn load(ctx: &Ctx, pool: Address, tag: &str) -> GatewayResult<(PoolState, TickMap)> {
    let gs = call::view(ctx, pool, &IAlgebraDirPool::globalStateCall {}, tag).await?;
    let liquidity = call::view(ctx, pool, &IAlgebraPool::liquidityCall {}, tag).await?;
    let tick = gs.tick.as_i32();
    let state = state_of(&gs, liquidity);
    Ok((state, tick_map(ctx, pool, tick, tag).await?))
}

/// Состояние пула из ответа globalState и liquidity. Принимает: разобранный globalState, ликвидность. Возвращает:
/// PoolState::Algebra (fee = feeZto, fee_otz = Some(feeOtz)). Ошибок нет. Нужна и предзагрузке
/// (prefetch_algebra_dir.rs), и тестам записи.
pub fn state_of(gs: &IAlgebraDirPool::globalStateReturn, liquidity: u128) -> PoolState {
    PoolState::Algebra {
        sqrt_price_x96: U256::from(gs.price),
        tick: gs.tick.as_i32(),
        liquidity,
        fee: u32::from(gs.feeZto),
        fee_otz: Some(u32::from(gs.feeOtz)),
    }
}
