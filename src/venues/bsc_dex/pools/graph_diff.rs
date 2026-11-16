//! Файл: venues/bsc_dex/pools/graph_diff.rs
//! Что делает: решение 308 (задача t137) — сверка накопительной карты графа с собранной с нуля КАК МНОЖЕСТВ: по каждой
//! паре токенов — отсортированный список (пул, ранг = глубина для ранжирования). Отличие — строка текста: пара, пулы,
//! которые есть только в одной карте, и пулы с разным рангом. Пустой итог — карты равны (инвариант И6 проекта
//! docs/notes/graph-snapshot-design-2026-11-16.md).
//! Кто вызывает: graph_full.rs (reconcile); тесты (tests/bsc_dex_graph_snapshot.rs).
//! От чего зависит: graph_work.rs (Picture).
//! Настройки: нет.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;

use super::graph_work::Picture;

/// Отличия карты `inc` (накопительная) от `full` (с нуля). Принимает: две картины. Возвращает: строки отличий (пусто —
/// равны). Ошибок нет.
pub fn diff(inc: &Picture, full: &Picture) -> Vec<String> {
    let pairs: BTreeSet<_> = inc.keys().chain(full.keys()).collect();
    let mut out = Vec::new();
    for pair in pairs {
        let a: BTreeMap<&String, &Decimal> = inc
            .get(pair)
            .into_iter()
            .flatten()
            .map(|x| (&x.0, &x.1))
            .collect();
        let b: BTreeMap<&String, &Decimal> = full
            .get(pair)
            .into_iter()
            .flatten()
            .map(|x| (&x.0, &x.1))
            .collect();
        if a == b {
            continue;
        }
        let only_a: Vec<&str> = a
            .keys()
            .filter(|k| !b.contains_key(*k))
            .map(|k| k.as_str())
            .collect();
        let only_b: Vec<&str> = b
            .keys()
            .filter(|k| !a.contains_key(*k))
            .map(|k| k.as_str())
            .collect();
        let ranks: Vec<String> = a
            .iter()
            .filter_map(|(k, x)| {
                b.get(k)
                    .filter(|y| **y != *x)
                    .map(|y| format!("{k}: {x} ≠ {y}"))
            })
            .collect();
        out.push(format!(
            "пара {}–{}: только в накопительной [{}]; только в собранной с нуля [{}]; разный ранг [{}]",
            pair.0,
            pair.1,
            only_a.join(", "),
            only_b.join(", "),
            ranks.join(", ")
        ));
    }
    out
}
