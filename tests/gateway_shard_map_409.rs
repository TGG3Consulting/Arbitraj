//! Файл: tests/gateway_shard_map_409.rs
//! Что делает: решение 409 (ревью t194) — карта по частям (gateway/shard_map.rs): вставка и чтение из многих потоков
//! (все записи на месте, обход видит каждую один раз), поиск по Borrow-ключу (&str для String), get_or_insert_with
//! при гонке отдаёт одно значение на ключ.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::shard_map. Моков нет — данные строятся в тесте.

use std::sync::Arc;

use arb::gateway::shard_map::ShardMap;

#[test]
fn concurrent_insert_get_and_for_each() {
    let m: Arc<ShardMap<u64, u64>> = Arc::new(ShardMap::default());
    let writers: Vec<_> = (0..4u64)
        .map(|t| {
            let m = m.clone();
            std::thread::spawn(move || {
                for i in 0..1_000u64 {
                    let k = t * 1_000 + i;
                    assert!(m.insert(k, k * 2).is_none());
                    assert_eq!(m.get(&k), Some(k * 2));
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    let mut n = 0u64;
    let mut sum = 0u64;
    m.for_each(|k, v| {
        assert_eq!(*v, k * 2);
        n += 1;
        sum += k;
    });
    assert_eq!(n, 4_000);
    assert_eq!(sum, (0..4_000u64).sum::<u64>());
    assert_eq!(m.insert(7, 1), Some(14), "замена отдаёт прежнее");
}

#[test]
fn borrow_lookup_and_single_value_on_race() {
    let m: ShardMap<String, u32> = ShardMap::default();
    m.insert("cake".to_string(), 1);
    assert_eq!(m.get("cake"), Some(1));
    assert_eq!(m.with("нет", |v| v.copied()), None);
    let shared: Arc<ShardMap<u32, Arc<u32>>> = Arc::new(ShardMap::default());
    let got: Vec<usize> = (0..8)
        .map(|i| {
            let s = shared.clone();
            std::thread::spawn(move || {
                Arc::as_ptr(&s.get_or_insert_with(5, || Arc::new(i))) as usize
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert!(got.iter().all(|p| *p == got[0]), "одно значение на ключ");
}
