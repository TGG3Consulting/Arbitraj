//! Файл: tests/core_pool_ref_multi.rs
//! Что делает: решение 295, шаг 1 — ссылка `PoolRef::Multi` (пара многомонетного пула): адрес вызова, физический пул
//! `site()`, свой контракт `own_contract()` у всех вариантов (у прежних — без изменений: site() = сама ссылка),
//! сериализация serde туда-обратно. Адреса — Nerve 3pool и его монеты из записи tests/data/nerve_2026-11-15.
//! Кто вызывает: cargo test --test core_pool_ref_multi.
//! От чего зависит: arb::core::types::PoolRef, tests/multi_common (записи).

mod multi_common;

use std::str::FromStr;

use alloy_primitives::{Address, B256};

use arb::core::types::PoolRef;

use multi_common::{pair, rec};

/// Пул и пара BUSD/USDT записи Nerve. Ошибок нет (паника теста).
fn nerve() -> (Address, (Address, Address)) {
    let probe = rec("nerve_2026-11-15", "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    (pool, pair("nerve_2026-11-15", "BUSD", "USDT"))
}

#[test]
fn site_and_own_contract_on_all_variants() {
    let (pool, (t0, t1)) = nerve();
    let multi = PoolRef::Multi {
        contract: pool,
        token0: t0,
        token1: t1,
    };
    assert_eq!(multi.address(), pool);
    assert_eq!(multi.site(), PoolRef::Contract(pool));
    assert_eq!(multi.own_contract(), Some(pool));
    // Прежние варианты: site() — та же ссылка, адрес — прежний.
    let contract = PoolRef::Contract(pool);
    assert_eq!(contract.site(), contract);
    assert_eq!(contract.own_contract(), Some(pool));
    let single = PoolRef::Singleton {
        manager: pool,
        id: B256::repeat_byte(7),
    };
    assert_eq!(single.site(), single);
    assert_eq!(single.own_contract(), None);
    assert_eq!(single.address(), pool);
    let member = PoolRef::Member {
        contract: pool,
        coin: t0,
    };
    assert_eq!(member.site(), member);
    assert_eq!(member.own_contract(), None);
    // Две пары одного пула — разные ссылки (разные записи реестра), но одна площадка.
    let (o0, o1) = pair("nerve_2026-11-15", "BUSD", "USDC");
    let other = PoolRef::Multi {
        contract: pool,
        token0: o0,
        token1: o1,
    };
    assert_ne!(multi, other);
    assert_eq!(multi.site(), other.site());
}

#[test]
fn serde_round_trip() {
    let (pool, (t0, t1)) = nerve();
    let multi = PoolRef::Multi {
        contract: pool,
        token0: t0,
        token1: t1,
    };
    let text = serde_json::to_string(&multi).unwrap();
    assert!(text.starts_with("{\"Multi\":{\"contract\":"), "{text}");
    assert_eq!(serde_json::from_str::<PoolRef>(&text).unwrap(), multi);
    // Прежний вид сериализации Contract не меняется.
    let c = serde_json::to_string(&PoolRef::Contract(pool)).unwrap();
    assert_eq!(c, format!("{{\"Contract\":{}}}", serde_json::to_string(&pool).unwrap()));
}
