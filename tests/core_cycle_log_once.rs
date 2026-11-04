//! Файл: tests/core_cycle_log_once.rs
//! Что делает: решение 139 (а) — журнал циклов DEX ↔ DEX не засыпается одной и той же причиной на всю площадку. Живой
//! журнал 04.11: «Цикл DEX ↔ DEX: сделки нет — dex_dex: нет кошелька DEX в маппинге» по нескольку записей в секунду —
//! память причины была по ключу цикла, а ключи стирались, как только цикл уходил из сигналов. Теперь причина площадки —
//! ключ venue_key: переживает retain (циклы пришли и ушли), повтор не пишется; смена причины и возврат причины после
//! «всё в порядке» — пишутся.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::cycle_state. Моков нет: проверяется сама память причин ядра.

use std::collections::HashSet;

use arb::core::cycle_state::{CycleState, venue_key};
use arb::core::types::VenueId;

#[test]
fn venue_reason_logged_once_per_change() {
    let st = CycleState::default();
    let v = VenueId("bsc_dex".into());
    let k = venue_key(&v, "wallet");
    let why = "dex_dex: нет кошелька DEX в маппинге (экран «Маппинг кошельков»)";
    assert!(st.fresh(&k, why), "первая запись пишется");
    // Блок за блоком циклы приходят и уходят — память причины площадки остаётся.
    for _ in 0..100 {
        st.retain("bsc_dex", &HashSet::new());
        assert!(!st.fresh(&k, why), "повтор той же причины не пишется");
    }
    // Кошелёк привязали (сброс без записи), потом снова отвязали — запись снова одна.
    st.fresh(&k, "");
    assert!(st.fresh(&k, why));
    assert!(!st.fresh(&k, why));
}

#[test]
fn cycle_keys_still_forgotten_on_retain() {
    let st = CycleState::default();
    let k = "bsc_dex:0xabc:p1>p2";
    assert!(st.fresh(k, "причина"));
    st.retain("bsc_dex", &HashSet::new());
    // Цикл ушёл из сигналов — память о нём стёрта (как было), пришёл снова — пишется.
    assert!(st.fresh(k, "причина"));
}
