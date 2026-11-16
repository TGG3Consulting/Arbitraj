//! Файл: core/mid_wake_route.rs
//! Что делает: решение 349 — чистая логика «какие монеты затрагивает событие пула» (без ядра, для mid_wake.rs и тестов).
//! Устройство маршрута адаптера DEX (venues/bsc_dex/quote/route_paths.rs шапка, routing.rs mids_ok): путь —
//! USDT → m1 → … → mk → монета X, внутренние узлы — только разрешённые промежуточные, k ≤ H − 1 (H — dex.route_max_hops в
//! режиме graph; классика — пара пулов через одну промежуточную, k ≤ 1). Пулы пути: «USDT — m1», «mi — mi+1» и «mk — X».
//! Пул «mk — X» (второй токен — не промежуточная и не USDT) есть только в маршрутах самой X — её будит событие по токенам
//! (tick.rs, ticker_index by_tokens). Поэтому стартовые промежуточные — только у пулов внутреннего участка: у пула оба
//! токена из промежуточных ∪ {USDT}, и стартовые — его промежуточные (ревью t160 Б1: пул «монета — WBNB» больше не будит
//! все монеты с пулом к WBNB). От стартовой промежуточной пула до mk — не больше H − 2 шагов по пулам «промежуточная —
//! промежуточная» (у пула «USDT — m1»: m1 … mk, k − 1 ≤ H − 2; у пула «mi — mi+1»: от mi+1 до mk меньше). Значит, будятся
//! все монеты с пулом к промежуточной, достижимой от стартовых за H − 2 шагов, и ни одна монета с маршрутом через пул не
//! теряется (проверка на записи — tests/bsc_dex_route_paths.rs mid_wake_covers_every_path).
//! Кто вызывает: core/mid_wake.rs, тесты (tests/core_mid_wake_349.rs, tests/bsc_dex_route_paths.rs).
//! От чего зависит: alloy_primitives.
//! Настройки: нет (шаги считает mid_wake.rs по dex.router_mode / dex.route_max_hops).

use alloy_primitives::Address;

/// Что делает: токены, монеты которых будит событие пула (см. шапку). Принимает: соседей токена по пулам площадки, токены
/// пула, промежуточные, шаги «промежуточная — промежуточная» (H − 2), USDT. Возвращает: токены — соседи достижимых
/// промежуточных, кроме USDT (пусто — пул не внутреннего участка маршрутов). Ошибок нет.
pub fn woken_tokens(
    peers: &dyn Fn(Address) -> Vec<Address>,
    pool: (Address, Address),
    mids: &[Address],
    depth: usize,
    usdt: Address,
) -> Vec<Address> {
    let inner = |t: Address| t == usdt || mids.contains(&t);
    if !inner(pool.0) || !inner(pool.1) {
        return Vec::new();
    }
    let start: Vec<Address> = [pool.0, pool.1]
        .into_iter()
        .filter(|t| mids.contains(t))
        .collect();
    let mut out = Vec::new();
    for r in reach(peers, &start, mids, depth) {
        out.extend(peers(r).into_iter().filter(|p| *p != usdt));
    }
    out
}

/// Что делает: промежуточные монеты, достижимые от `start` по пулам «промежуточная — промежуточная» не больше чем за
/// `depth` шагов (поиск в ширину). Принимает: соседей токена, стартовые промежуточные, список промежуточных, шаги.
/// Возвращает: достижимые (со стартовыми). Ошибок нет.
pub fn reach(
    peers: &dyn Fn(Address) -> Vec<Address>,
    start: &[Address],
    mids: &[Address],
    depth: usize,
) -> Vec<Address> {
    let mut seen: Vec<Address> = start.to_vec();
    let mut front: Vec<Address> = start.to_vec();
    for _ in 0..depth {
        let mut next = Vec::new();
        for m in &front {
            for p in peers(*m) {
                if mids.contains(&p) && !seen.contains(&p) {
                    seen.push(p);
                    next.push(p);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        front = next;
    }
    seen
}
