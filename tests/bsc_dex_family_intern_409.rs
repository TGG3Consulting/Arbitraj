//! Файл: tests/bsc_dex_family_intern_409.rs
//! Что делает: решение 409 (t194) — таблица видов DEX без замка на чтение (pools/family_intern.rs): поведение
//! families::make прежнее — то же (id, вариант dex.kind) даёт ту же запись &'static Family (dex_rules::still сравнивает
//! указатели), в том числе при одновременных вызовах из многих потоков; другой id или вариант с другим протоколом —
//! другая запись; неизвестный вариант и «не поддерживается» — None.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::families, arb::settings::catalog::dex (DEX_KINDS). Моков нет.

use arb::settings::catalog::dex::DEX_KINDS;
use arb::venues::bsc_dex::pools::families;

#[test]
fn same_pair_same_family_across_threads() {
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                DEX_KINDS
                    .iter()
                    .filter(|k| **k != "unsupported")
                    .map(|k| {
                        let f = families::make(&format!("t409-{k}"), k)
                            .expect("вариант каталога понятен");
                        f as *const families::Family as usize
                    })
                    .collect::<Vec<usize>>()
            })
        })
        .collect();
    let all: Vec<Vec<usize>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for other in &all[1..] {
        assert_eq!(&all[0], other, "одна запись на сочетание при гонке потоков");
    }
    for k in DEX_KINDS.iter().filter(|k| **k != "unsupported") {
        let id = format!("t409-{k}");
        let a = families::make(&id, k).unwrap();
        let b = families::make(&id, k).unwrap();
        assert!(std::ptr::eq(a, b), "{k}: повторный вызов — та же запись");
        assert_eq!(a.id, id);
        assert_eq!(a.title, id);
    }
}

#[test]
fn different_id_or_proto_different_family() {
    let a = families::make("t409-a", "pancakeswap_v2").unwrap();
    let b = families::make("t409-b", "pancakeswap_v2").unwrap();
    assert!(!std::ptr::eq(a, b));
    let c = families::make("t409-a", "pancakeswap_v3").unwrap();
    assert!(!std::ptr::eq(a, c));
    assert_ne!(a.proto, c.proto);
    assert!(std::ptr::eq(
        a,
        families::make("t409-a", "pancakeswap_v2").unwrap()
    ));
    assert!(families::make("t409-a", "unsupported").is_none());
    assert!(families::make("t409-a", "нет такого").is_none());
}
