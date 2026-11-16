//! Файл: tests/core_mid_wake_349.rs
//! Что делает: решение 349 — какие промежуточные монеты достижимы от пула события (core/mid_wake.rs reach): чистая
//! логика поиска в ширину по пулам «промежуточная — промежуточная» с пределом шагов (dex.route_max_hops − 2).
//! Адреса — настоящие контракты BSC (WBNB, USDT, USDC, BTCB, CAKE); связи пулов заданы списком соседей.
//! Проверяет: (1) 0 шагов (классика, 2 хопа) — только промежуточные пула; (2) 1 шаг — соседняя промежуточная через пул
//! «промежуточная — промежуточная», не промежуточные соседи (монеты, USDT) в достижимые не входят; (3) 2 шага — дальше
//! по цепочке; (4) шагов больше, чем связей, — поиск кончается, без повторов.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::mid_wake, alloy_primitives.
//! Моков нет: площадки не участвуют.

use alloy_primitives::{Address, address};
use arb::core::mid_wake::reach;

const WBNB: Address = address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const USDT: Address = address!("55d398326f99059fF775485246999027B3197955");
const USDC: Address = address!("8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d");
const BTCB: Address = address!("7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c");
const CAKE: Address = address!("0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");

/// Соседи по пулам: WBNB — USDT, CAKE, USDC; USDC — USDT, WBNB, BTCB; BTCB — USDC.
fn peers(t: Address) -> Vec<Address> {
    if t == WBNB {
        vec![USDT, CAKE, USDC]
    } else if t == USDC {
        vec![USDT, WBNB, BTCB]
    } else if t == BTCB {
        vec![USDC]
    } else {
        vec![]
    }
}

#[test]
fn reach_by_depth() {
    let mids = [WBNB, USDC, BTCB];
    assert_eq!(reach(&peers, &[WBNB], &mids, 0), vec![WBNB]);
    assert_eq!(reach(&peers, &[WBNB], &mids, 1), vec![WBNB, USDC]);
    assert_eq!(reach(&peers, &[WBNB], &mids, 2), vec![WBNB, USDC, BTCB]);
    assert_eq!(reach(&peers, &[WBNB], &mids, 10), vec![WBNB, USDC, BTCB]);
    // Пул «промежуточная — промежуточная» (WBNB–USDC): обе стартовые, без повторов.
    assert_eq!(
        reach(&peers, &[WBNB, USDC], &mids, 1),
        vec![WBNB, USDC, BTCB]
    );
}
