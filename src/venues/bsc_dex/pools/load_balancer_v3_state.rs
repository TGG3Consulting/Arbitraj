//! Файл: venues/bsc_dex/pools/load_balancer_v3_state.rs
//! Что делает: решение 441 — сборка состояния пула Balancer V3 (PoolState::VaultPool с частью V3) из разобранных
//! ответов пачки чтения и проверка согласованности (длины токенов / балансов / множителей / курсов / весов, монеты
//! пары среди токенов, комиссия < 100 %). Вынесено из load_balancer_v3.rs (предел размера файла). Без сети.
//! Кто вызывает: pools/load_balancer_v3.rs (decode; Parts и state реэкспортирует).
//! От чего зависит: core/types (PoolState, VaultPoolData, VaultV3, VaultCurve, VaultV3Rule).
//! Настройки: нет.

use alloy_primitives::{Address, U256};

use crate::core::types::pool::PoolState;
use crate::core::types::pool_vault::{VaultCurve, VaultPoolData, VaultV3, VaultV3Rule};

/// Части состояния пула без части V3 (сборка — state).
pub struct Parts<'a> {
    /// Адрес пула (id пула V3 — его адрес).
    pub pool: Address,
    /// Токены пула в порядке хранилища (getPoolTokenInfo).
    pub tokens: &'a [Address],
    /// Сырые балансы хранилища в единицах токена (getPoolTokenInfo balancesRaw).
    pub balances: Vec<U256>,
    /// Множители точности 10^(18 − decimals) (getPoolTokenRates decimalScalingFactors).
    pub scaling: Vec<U256>,
    /// Статическая комиссия обмена, 18 знаков (getPoolConfig staticSwapFeePercentage).
    pub fee: U256,
    /// Пауза пула или хранилища, либо пул не инициализирован (обменов нет).
    pub paused: bool,
    /// Кривая пула и её параметры.
    pub curve: VaultCurve,
}

/// Сборка состояния (без сети). Принимает: части, часть V3, пару. Возвращает: VaultPool. Ошибки (текст): длины токенов
/// / балансов / множителей / курсов / весов / нижних балансов не совпали, монеты пары нет среди токенов, комиссия ≥ 100 %.
pub fn state(p: Parts, v3: VaultV3, pair: (Address, Address)) -> Result<PoolState, String> {
    let n = p.tokens.len();
    let weights_ok = match &p.curve {
        VaultCurve::Weighted { weights } => weights.len() == n,
        // Решение 452: Gyro, ReClamm, LBP с фиксированной ценой — ровно два токена.
        VaultCurve::V3 => n == 2,
        VaultCurve::Stable { .. } => true,
    };
    let min_ok = match &v3.rule {
        VaultV3Rule::WeightedMin { min } => min.len() == n,
        // Решение 452: LBP — два токена; нижние балансы — только у выпуска 4.
        VaultV3Rule::Lbp(l) => n == 2 && (l.min.is_empty() || l.min.len() == n),
        _ => true,
    };
    let lens = [p.balances.len(), p.scaling.len(), v3.live.len(), v3.rates.len()];
    if n < 2 || lens.iter().any(|l| *l != n) || !weights_ok || !min_ok {
        return Err(format!(
            "длины токенов / балансов / множителей / курсов / весов не совпали (токенов {n})"
        ));
    }
    if p.fee >= U256::from(10u8).pow(U256::from(18u8)) {
        return Err(format!("комиссия {} ≥ 1e18", p.fee));
    }
    let index = |t: Address| {
        p.tokens
            .iter()
            .position(|x| *x == t)
            .map(|i| i as u8)
            .ok_or(format!("монеты {t} нет среди токенов пула"))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    Ok(PoolState::VaultPool(Box::new(VaultPoolData {
        // Id пула V3 — его адрес (topic1 события Swap хранилища — адрес пула, IVaultEvents.sol L53-L61).
        pool_id: p.pool.into_word(),
        balances: p.balances,
        scaling: p.scaling,
        fee: p.fee,
        curve: p.curve,
        paused: p.paused,
        i0,
        i1,
        v3: Some(Box::new(v3)),
    })))
}
