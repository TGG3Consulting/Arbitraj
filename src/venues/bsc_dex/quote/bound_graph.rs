//! Файл: venues/bsc_dex/quote/bound_graph.rs
//! Что делает: решение 361 — верх ρ курса по набору пулов (quote/bound.rs): наибольшее произведение r_e
//! (quote/bound_rate.rs) по путям ≤ H хопов (динамика по слоям, повторный заход в промежуточную разрешён — путей только
//! больше). Строго, потому что монета и USDT в путях не внутренние (route_paths.rs: без повторного захода, внутренние —
//! промежуточные) ⇒ пулы монеты и пулы с USDT идут только в одну сторону, а обмены одного направления в одном пуле только
//! ухудшают его цену (каждый ≤ r_e·вход). При H ≥ 3 пул между двумя промежуточными (встречные доли — строгой границы в
//! коде нет) — метод не действует. Доказательство — docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2.
//! Выделено из bound.rs (ревью t162, Н10: файл ≤ 200 строк).
//! Кто вызывает: quote/bound.rs, тесты.
//! От чего зависит: quote/bound_rate.rs, pools/registry.rs (Entry), ctx.rs.
//! Настройки: через bound_rate — dex.v2_fee_pct [вид пула V2].

use std::collections::{HashMap, HashSet};

use alloy_primitives::Address;
use rust_decimal::Decimal;

use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::bound_rate::{ceil_dec, mul_up, rate};

/// Что делает: верх отдачи `to` за единицу `from` (минимальные единицы) по путям ≤ hops хопов через пулы `pools`
/// (динамика по слоям, Якоби). Внутренние узлы — промежуточные; рёбра в `from` и из `to` не берутся.
/// Принимает: окружение, пулы, откуда, куда, промежуточные, предел хопов. Возвращает: верх.
/// Ошибки: (класс, причина) — пул без строгого r_e (вид, крючок), пул между промежуточными при hops ≥ 3, пути нет.
pub fn rho_raw(
    ctx: &Ctx,
    pools: &[Entry],
    from: Address,
    to: Address,
    mids: &[Address],
    hops: usize,
) -> Result<Decimal, (&'static str, String)> {
    let mid: HashSet<Address> = mids.iter().copied().collect();
    let mut edges: Vec<(Address, Address, Decimal)> = Vec::new();
    for e in pools {
        let (a, b) = (e.pool.token0, e.pool.token1);
        if hops >= 3 && mid.contains(&a) && mid.contains(&b) {
            return Err((
                "mid_pool",
                format!(
                    "{}: пул между промежуточными при маршруте ≥ 3 хопов",
                    e.family.title
                ),
            ));
        }
        for (i, o) in [(a, b), (b, a)] {
            if i == to || o == from {
                continue;
            }
            let r = rate(ctx, e, i).map_err(|w| ("pool_kind", w))?;
            let r = ceil_dec(r)
                .ok_or_else(|| ("overflow", "верх пула не помещается в число".to_string()))?;
            edges.push((i, o, r));
        }
    }
    let mut v: HashMap<Address, Decimal> = HashMap::from([(to, Decimal::ONE)]);
    for _ in 0..hops {
        let mut next = v.clone();
        for (i, o, r) in &edges {
            let Some(w) = v.get(o) else { continue };
            let Some(c) = mul_up(*r, *w) else {
                return Err(("overflow", "верх пути не помещается в число".into()));
            };
            let cur = next.entry(*i).or_insert(c);
            if c > *cur {
                *cur = c;
            }
        }
        v = next;
    }
    v.get(&from)
        .copied()
        .ok_or_else(|| ("no_route", "пути по пулам набора нет".into()))
}
