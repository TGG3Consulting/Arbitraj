//! Файл: venues/bsc_dex/exec/quoter_refetch_note.rs
//! Что делает: решение 382 — чем запись пула в памяти бота отличается от того же пула, перечитанного с узла на голове
//! H после расхождения расчёта с квотером перед сделкой (exec/quoter_refetch.rs). Чистая логика, без узла: верхнее
//! состояние (PoolState — цена, тик, ликвидность / резервы) и карта тиков V3/CL в ОБЩИХ прочитанных словах битмапа
//! (охват карты зависит от момента чтения — сравнивать можно только слова, прочитанные и там, и там). Строка уходит в
//! журнал dex.memory_refetch: по ней видно, что именно было неверно в памяти — верх (пропущенное или ещё не применённое
//! событие Swap / Sync) или карта тиков (пропущенный Mint / Burn) — без чтения узла задним числом.
//! differs — доказано ли расхождение (тогда пул ставится на полное чтение, pools/settle_reread.rs).
//! Кто вызывает: exec/quoter_refetch.rs, check/quoter_one.rs; тесты (tests/bsc_dex_quoter_refetch_382.rs).
//! От чего зависит: pools/registry.rs (Entry), pools/ticks.rs (TickMap).
//! Настройки: нет.

use std::collections::BTreeSet;

use super::super::pools::registry::Entry;
use super::super::pools::ticks::TickMap;

/// Расхождение карт тиков в общих словах: (первое слово, последнее слово) общих слов, тики с разными данными
/// (есть только в одной карте или разные gross / net). None у слов — окна не пересекаются.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickDiff {
    pub words: Option<(i16, i16)>,
    pub ticks: Vec<i32>,
}

/// Расхождение карт тиков памяти и узла в общих словах битмапа. Принимает: карту памяти, карту узла. Возвращает:
/// TickDiff (окна без общих слов или разный шаг тиков — карты не сравниваются: words = None). Ошибок нет.
pub fn tick_diff(mem: &TickMap, fresh: &TickMap) -> TickDiff {
    let (lo, hi) = (
        mem.words.0.max(fresh.words.0),
        mem.words.1.min(fresh.words.1),
    );
    if lo > hi || mem.spacing != fresh.spacing {
        return TickDiff {
            words: None,
            ticks: Vec::new(),
        };
    }
    let inside = |t: &i32| (lo..=hi).contains(&TickMap::word_of(*t, mem.spacing));
    let all: BTreeSet<i32> = mem
        .ticks
        .keys()
        .chain(fresh.ticks.keys())
        .copied()
        .filter(inside)
        .collect();
    let ticks = all
        .into_iter()
        .filter(|t| mem.ticks.get(t) != fresh.ticks.get(t))
        .collect();
    TickDiff {
        words: Some((lo, hi)),
        ticks,
    }
}

/// Доказано ли, что память не равна узлу: верх состояния различается или карта тиков в общих словах не совпадает.
/// Принимает: запись памяти, запись, перечитанную на блоке. Ошибок нет.
pub fn differs(mem: &Entry, fresh: &Entry) -> bool {
    if mem.pool.state != fresh.pool.state {
        return true;
    }
    match (&mem.ticks, &fresh.ticks) {
        (Some(m), Some(f)) => !tick_diff(m, f).ticks.is_empty(),
        _ => false,
    }
}

/// Строка «чем память отличается от узла» для журнала. Принимает: запись памяти, запись, перечитанную на блоке, блок.
/// Возвращает: текст. Ошибок нет.
pub fn note(mem: &Entry, fresh: &Entry, block: u64) -> String {
    let at = format!(
        "{} {}: память — последнее событие блок {}{}",
        mem.family.title,
        mem.ticker,
        mem.synced_block,
        mem.synced_log
            .map_or(String::new(), |i| format!(", logIndex {i}"))
    );
    let top = if mem.pool.state == fresh.pool.state {
        "верх состояния совпадает с узлом".to_string()
    } else {
        format!(
            "верх состояния НЕ совпадает с узлом на блоке {block}: память {:?}, узел {:?}",
            mem.pool.state, fresh.pool.state
        )
    };
    let map = match (&mem.ticks, &fresh.ticks) {
        (Some(m), Some(f)) => ticks_text(&tick_diff(m, f), m, f),
        _ => String::new(),
    };
    format!("{at}; {top}{map}")
}

/// Текст расхождения карт тиков (все расходящиеся тики с данными памяти и узла). Ошибок нет.
fn ticks_text(d: &TickDiff, m: &TickMap, f: &TickMap) -> String {
    let Some((lo, hi)) = d.words else {
        return "; карты тиков не сравнимы (общих прочитанных слов нет)".into();
    };
    if d.ticks.is_empty() {
        return format!("; карта тиков в общих словах {lo}..{hi} совпадает");
    }
    let shown: Vec<String> = d
        .ticks
        .iter()
        .map(|t| {
            format!(
                "тик {t}: память {:?}, узел {:?}",
                m.ticks.get(t),
                f.ticks.get(t)
            )
        })
        .collect();
    format!(
        "; карта тиков в общих словах {lo}..{hi} НЕ совпадает: {} тик(ов) — {}",
        d.ticks.len(),
        shown.join("; ")
    )
}
