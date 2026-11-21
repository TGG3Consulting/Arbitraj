//! Файл: tests/xt_symbols_live.rs
//! Что проверяет: решение 190 — справочник пар XT (venues/xt/symbols_parse.rs) на ЖИВОМ ответе 06.11.2026
//! (tests/data/xt_live_2026-11-06/symbol_pepe.json): у pepe_usdt фильтр QUANTITY max = 10^30 (больше предела Decimal)
//! — разбор не падает, предел max = нет ограничения, min и шаг сохраняются; соседняя пара btc_usdt разобрана.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::xt::symbols_parse, serde_json.

use rust_decimal::Decimal;

use arb::venues::xt::symbols_parse::parse_list;

#[test]
fn huge_max_is_unbounded_and_list_parses() {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/xt_live_2026-11-06/symbol_pepe.json"
    );
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let parsed = parse_list(&v, "USDT").expect("справочник разобран");
    let map = parsed.map.expect("список пар");
    assert_eq!(map.len(), 2);
    let pepe = map.get("PEPE").expect("PEPE");
    assert_eq!(pepe.qty.max, None, "10^30 — без ограничения");
    assert_eq!(pepe.qty.min, Some(Decimal::from(10)));
    assert_eq!(pepe.qty.tick, Some(Decimal::from(10)));
    assert!(map.contains_key("BTC"));
}
