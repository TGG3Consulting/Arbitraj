//! Файл: venues/bsc_dex/pools/ekubo_accept.rs
//! Что делает: правила «берём ли пул Ekubo по его ключу» (решения 447, 454) — общие для проверки пула из справочника
//! (verify_ekubo.rs: ключ у PoolKeyIndex) и пула из истории / события PoolInitialized ядра (history_ekubo.rs): PoolId =
//! keccak256 ключа; расширение — без него или официальное из настроек (ekubo_ext.rs); token0 — нативный ETH
//! сопоставляется с обёрткой dex.native_wrapped (ekubo_native.rs). Итог — найденный пул (пара графа по адресу) и ключ
//! для памяти ключей (ekubo_keys.rs). Без сети.
//! Кто вызывает: pools/verify_ekubo.rs, pools/history_ekubo.rs.
//! От чего зависит: ctx.rs, abi_ekubo.rs (pool_id), ekubo_ext.rs, ekubo_native.rs, ekubo_keys.rs, math/ekubo_config.rs,
//! discover.rs (Found).
//! Настройки: dex.native_wrapped [площадка DEX]; через ekubo_ext.rs — dex.ekubo_mev_capture, dex.ekubo_twamm,
//! dex.ekubo_oracle, dex.ekubo_boosted_fees [DEX].
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/types/poolKey.sol L34-L39 (PoolId), src/types/poolConfig.sol
//! L41-L45 (расширение), L58-L84 (вид пула).

use alloy_primitives::{Address, B256};

use crate::core::types::PoolRef;

use super::super::ctx::Ctx;
use super::super::math::ekubo_config::{self, Shape};
use super::abi_ekubo::pool_id;
use super::discover::Found;
use super::ekubo_keys::Key;
use super::families::Family;
use super::{ekubo_ext, ekubo_native};

/// Ключ пула ядра.
#[derive(Debug, Clone, Copy)]
pub struct PoolKeyIn {
    pub token0: Address,
    pub token1: Address,
    pub config: B256,
}

/// Пул по ключу. Принимает: окружение, вид, ядро, PoolId, ключ. Возвращает: найденный пул (coin — token0 пары графа;
/// вызывающий уточняет монету) и ключ для памяти. Ошибки (текст): PoolId не тот, расширение не поддержано, нативная
/// монета без обёртки.
pub fn accept(
    ctx: &Ctx,
    f: &'static Family,
    core: Address,
    id: B256,
    k: PoolKeyIn,
) -> Result<(Found, Key), String> {
    if pool_id(k.token0, k.token1, k.config) != id {
        return Err(format!("ключ пула даёт другой PoolId, а не {id}"));
    }
    ekubo_ext::classify(ctx, f.id, ekubo_config::extension(&k.config))?;
    let wrapped = ctx.addr("dex.native_wrapped").ok();
    let (t0, t1, native) = ekubo_native::pair(k.token0, k.token1, wrapped)?;
    let spacing = match ekubo_config::shape(&k.config) {
        Shape::Concentrated { spacing } => Some(spacing as i32),
        Shape::FullRange | Shape::Stable { .. } => None,
    };
    let found = Found {
        family: f,
        reference: PoolRef::Singleton { manager: core, id },
        token0: t0,
        token1: t1,
        coin: t0,
        fee_pips: None,
        tick_spacing: spacing,
        bin_step: None,
        key: None,
        hooks: None,
    };
    let key = Key {
        config: k.config,
        native: native.is_some(),
    };
    Ok((found, key))
}
