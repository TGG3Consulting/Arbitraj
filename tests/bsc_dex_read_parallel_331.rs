//! Файл: tests/bsc_dex_read_parallel_331.rs
//! Что делает: решение 331 — настройка одновременного перечтения пулов маршрута перед сделкой
//! (verify.read_parallel_max) в каталоге: целая, необязательная, только площадка DEX.
//! Чего здесь нет (нужна запись с живого бота, docs/TASKS.md t153): ответы узла на одновременные eth_call видов «по
//! одному» одного блока и время verify с настройкой и без — само чтение (pools/load_batch_par.rs) идёт к узлу.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::{catalog, schema}.
//! Настройки: нет.

use arb::settings::catalog;
use arb::settings::schema::{Kind, VenueFor};

#[test]
fn catalog_has_331_setting() {
    let name = "verify.read_parallel_max";
    let def = catalog::find(name).unwrap_or_else(|| panic!("{name} в каталоге"));
    assert!(def.optional, "{name}: необязательная");
    assert_eq!(def.kind, Kind::Integer, "{name}: целое");
    assert!(
        matches!(def.venue_for, VenueFor::Dex),
        "{name}: только площадка DEX"
    );
}
