//! Файл: venues/bsc_dex/exec/executor/kind_key.rs
//! Что делает: решение 395 (t181) — ключ пула внутри общего менеджера для шагов исполнителя v3: UNI_V4 (Uniswap V4) и
//! INF_CL / INF_BIN (PancakeSwap Infinity CL / Bin; Infinity Stable — ключ пула CL с крючком). Copy-копии ключей реестра
//! pools/key.rs (Params шага копируется), перевод туда и обратно; ABI-кодирование ключа и PoolId — pools/key.rs (то же
//! кодирование кладётся в data шага: abi.encode(PoolKey) статической структуры — слова подряд). Проверка «токены шага —
//! валюты ключа» до отправки (контракт иначе откатит «key» / «native»).
//! Кто вызывает: exec/executor/{kind, entry}.rs.
//! От чего зависит: pools/key.rs.
//! Настройки: нет.

use alloy_primitives::aliases::I24;
use alloy_primitives::{Address, B256};

use super::super::super::pools::key::{InfinityPoolKeyData, PoolKey, UniswapPoolKeyData};

/// Ключ пула Uniswap V4 для шага UNI_V4 (поля PoolKey v4-core src/types/PoolKey.sol; копия pools/key.rs
/// UniswapPoolKeyData, но Copy — Params копируется).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V4Key {
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: I24,
    pub hooks: Address,
}

/// Ключ пула PancakeSwap Infinity для шагов INF_CL / INF_BIN (поля PoolKey infinity-core src/types/PoolKey.sol).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfKey {
    pub currency0: Address,
    pub currency1: Address,
    pub hooks: Address,
    pub manager: Address,
    pub fee: u32,
    pub parameters: B256,
}

impl V4Key {
    /// Из ключа реестра (pools/key.rs). Ошибок нет.
    pub fn of(k: &UniswapPoolKeyData) -> V4Key {
        V4Key {
            currency0: k.currency0,
            currency1: k.currency1,
            fee: k.fee,
            tick_spacing: k.tick_spacing,
            hooks: k.hooks,
        }
    }

    /// Ключ реестра (для ABI и PoolId — pools/key.rs). Ошибок нет.
    pub fn pool_key(&self) -> PoolKey {
        PoolKey::Uniswap(UniswapPoolKeyData {
            currency0: self.currency0,
            currency1: self.currency1,
            fee: self.fee,
            tick_spacing: self.tick_spacing,
            hooks: self.hooks,
        })
    }
}

impl InfKey {
    /// Из ключа реестра (pools/key.rs). Ошибок нет.
    pub fn of(k: &InfinityPoolKeyData) -> InfKey {
        InfKey {
            currency0: k.currency0,
            currency1: k.currency1,
            hooks: k.hooks,
            manager: k.manager,
            fee: k.fee,
            parameters: k.parameters,
        }
    }

    /// Ключ реестра (для ABI и PoolId — pools/key.rs). Ошибок нет.
    pub fn pool_key(&self) -> PoolKey {
        PoolKey::Infinity(InfinityPoolKeyData {
            currency0: self.currency0,
            currency1: self.currency1,
            hooks: self.hooks,
            manager: self.manager,
            fee: self.fee,
            parameters: self.parameters,
        })
    }
}

/// Токены шага — ровно валюты ключа (в любом порядке), обе — ERC-20 (нативная валюта address 0 шагом исполнителя не
/// идёт: контракт — «native»). Принимает: валюты ключа, токены шага. Возвращает: Ok или причину. Ошибок иных нет.
pub fn check_tokens(
    (c0, c1): (Address, Address),
    token_in: Address,
    token_out: Address,
) -> Result<(), String> {
    if c0.is_zero() || c1.is_zero() {
        return Err(
            "в ключе пула нативная валюта (address 0) — шаг исполнителя только для ERC-20".into(),
        );
    }
    let same = (token_in == c0 && token_out == c1) || (token_in == c1 && token_out == c0);
    if !same {
        return Err(format!(
            "токены шага {token_in:#x} → {token_out:#x} — не валюты ключа пула ({c0:#x}, {c1:#x})"
        ));
    }
    Ok(())
}
