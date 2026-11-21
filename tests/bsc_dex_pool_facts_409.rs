//! Файл: tests/bsc_dex_pool_facts_409.rs
//! Что делает: решение 409 (t194, п. а) — факты пулов проверок без замка на чтение (check/pool_facts_store.rs):
//! поведение Checks то же — правка сохраняется, даже если вернула «не записано» (как entry().or_default()), поколение
//! растёт только при «записано»; pool и pool_arc отдают одно и то же; копия на момент снимка (pinned, решение 239) не
//! видит последующих правок; параллельные читатели видят факты целиком, пока писатель пишет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::state, arb::core::types. Моков нет — данные строятся в тесте.

use alloy_primitives::Address;
use arb::core::types::PoolRef;
use arb::venues::bsc_dex::check::state::Checks;

fn pool(n: u8) -> PoolRef {
    PoolRef::Contract(Address::repeat_byte(n))
}

#[test]
fn update_semantics_and_pinned_copy() {
    let c = Checks::default();
    let r = pool(1);
    assert!(c.pool(&r).gas_buy.is_none());
    let g0 = c.generation();
    // «Не записано» — правка всё равно сохраняется, поколение не растёт.
    assert!(!c.update_pool_if(&r, |f| {
        f.gas_buy = Some(100);
        false
    }));
    assert_eq!(c.pool(&r).gas_buy, Some(100));
    assert_eq!(c.generation(), g0);
    c.update_pool(&r, |f| f.gas_sell = Some(200));
    assert_eq!(c.generation(), g0 + 1);
    let a = c.pool_arc(&r);
    assert_eq!((a.gas_buy, a.gas_sell), (Some(100), Some(200)));
    let pinned = c.pinned(&[], [&r]);
    c.update_pool(&r, |f| f.gas_buy = Some(300));
    assert_eq!(c.pool(&r).gas_buy, Some(300));
    assert_eq!(
        pinned.pool(&r).gas_buy,
        Some(100),
        "копия не видит новых правок"
    );
    // Ранее взятый Arc — прежние факты целиком.
    assert_eq!(a.gas_buy, Some(100));
    assert!(pinned.pool(&pool(2)).gas_buy.is_none());
    assert_eq!(c.pool_taxes().len(), 1);
}

#[test]
fn readers_see_whole_facts_while_writing() {
    let c = Checks::default();
    let r = pool(3);
    c.update_pool(&r, |f| {
        f.gas_buy = Some(0);
        f.gas_sell = Some(0);
    });
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let (c, r) = (c.clone(), r.clone());
            std::thread::spawn(move || {
                let mut last = 0u64;
                for _ in 0..20_000 {
                    let f = c.pool_arc(&r);
                    let (b, s) = (f.gas_buy.unwrap(), f.gas_sell.unwrap());
                    assert_eq!(b, s, "факты пула целиком");
                    assert!(b >= last, "не назад");
                    last = b;
                }
            })
        })
        .collect();
    for i in 1..=2_000u64 {
        c.update_pool(&r, |f| {
            f.gas_buy = Some(i);
            f.gas_sell = Some(i);
        });
    }
    for t in readers {
        t.join().unwrap();
    }
    assert_eq!(c.pool(&r).gas_buy, Some(2_000));
}
