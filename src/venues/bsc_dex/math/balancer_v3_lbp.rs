//! Файл: venues/bsc_dex/math/balancer_v3_lbp.rs
//! Что делает: решение 452 — onSwap пулов Liquidity Bootstrapping (LBP) Balancer V3 по выпуску: окно продажи (вне окна —
//! откат), запрет продажи токена проекта в пул (выход должен быть токеном проекта), виртуальный баланс резерва (v3, v4),
//! весовой расчёт WeightedPool своего выпуска; и LBP с фиксированной ценой (только покупка токена проекта по цене).
//! Веса — на блоке чтения (getNormalizedWeights пула считает их по времени блока тем же _getNormalizedWeights, что onSwap).
//! Кто вызывает: math/balancer_v3_pool.rs.
//! От чего зависит: core/types (LbpState, LbpVer), math/balancer_v3_pool.rs (weighted), math/balancer_math.rs.
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info (contracts/lbp/LBPool.sol):
//! - v3/deprecated/20250307-v3-liquidity-bootstrapping-pool L289-L305 и 20250701-v3-liquidity-bootstrapping-pool-v2 —
//!   окно, запрет, super.onSwap = WeightedPool.sol без «+ 1» (onSwap = 20241205, сверено по тексту);
//! - v3/deprecated/20251219-v3-liquidity-bootstrapping-pool-v3 LBPool.onSwap: окно, запрет; виртуальный резерв 0 —
//!   super.onSwap; иначе баланс резерва + виртуальный, super.onSwap (WeightedPool 20241205), при выходе резерва
//!   «расчёт > реальный резерв» — откат InsufficientRealReserveBalance (расчёт — при любом виде обмена);
//! - v3/tasks/20260501-v3-liquidity-bootstrapping-pool-v4 LBPool.onSwap: то же, WeightedPool = 20260115 («+ 1», нижние
//!   балансы getMinTokenBalances), проверка выхода резерва: точный вход — расчёт, точный выход — заданная сумма;
//! - v3/tasks/20251205-v3-fixed-price-lbp contracts/lbp/FixedPriceLBPool.sol onSwap: окно; вход токена проекта —
//!   откат; точный вход — сумма divDown цена, точный выход — сумма mulUp цена.

use alloy_primitives::U256;

use crate::core::types::pool_vault::{LbpState, LbpVer};

use super::balancer_math::{div_down, mul_up};
use super::balancer_v3_pool::weighted;

/// LBP onSwap. Принимает: состояние LBP, веса, живые балансы, индексы, сумму (18 знаков), точный ли вход.
/// Возвращает: расчёт пула. None — окно закрыто, продажа токена проекта запрещена, откат весового расчёта или проверки
/// реального резерва.
pub fn on_swap(
    l: &LbpState,
    weights: &[U256],
    bal: &[U256],
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
) -> Option<U256> {
    let project = usize::from(l.project);
    if !l.enabled || (l.block_in && j != project) {
        return None;
    }
    let min = match l.ver {
        LbpVer::VirtualMin => Some(l.min.as_slice()),
        LbpVer::Plain | LbpVer::Virtual => None,
    };
    if l.ver == LbpVer::Plain || l.virt.is_zero() {
        return weighted(weights, bal, i, j, a, exact_in, min);
    }
    // Два токена: резерв — не токен проекта.
    let reserve = 1usize.checked_sub(project)?;
    let real = *bal.get(reserve)?;
    let mut b = bal.to_vec();
    *b.get_mut(reserve)? = real.checked_add(l.virt)?;
    let calc = weighted(weights, &b, i, j, a, exact_in, min)?;
    if j == reserve {
        let out = match l.ver {
            // v4: точный вход — расчёт, точный выход — заданная сумма.
            LbpVer::VirtualMin if !exact_in => a,
            // v3: сравнивается расчёт при любом виде обмена (как в исходнике).
            _ => calc,
        };
        if out > real {
            return None;
        }
    }
    Some(calc)
}

/// LBP с фиксированной ценой (FixedPriceLBPool.onSwap). Принимает: окно открыто ли, индекс токена проекта, цену (18
/// знаков), индекс входа, сумму, точный ли вход. Возвращает: точный вход — токен проекта на выходе, точный выход —
/// резерв на входе. None — окно закрыто, вход — токен проекта, деление на ноль, переполнение.
pub fn fixed(
    enabled: bool,
    project: u8,
    rate: U256,
    i: usize,
    a: U256,
    exact_in: bool,
) -> Option<U256> {
    if !enabled || i == usize::from(project) {
        return None;
    }
    if exact_in {
        div_down(a, rate)
    } else {
        mul_up(a, rate)
    }
}
