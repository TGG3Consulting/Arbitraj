//! Файл: venues/bsc_dex/check/hook_batch_parse.rs
//! Что делает: чистые части пакетной пробы крючков перед сделкой (check/hook_batch.rs, решение «335 (В1, пакет)»):
//! - classify: разбор отказа вызова квотера в пакете. Пакет идёт через Multicall3.aggregate3 (allowFailure; откат —
//!   «вызов откатился: 0x…», node/multicall.rs decode) или пачкой JSON-RPC (откат — «… execution reverted: 0x…»,
//!   errors.rs from_rpc). UnexpectedRevertBytes(NotEnoughLiquidity(PoolId)) — «не хватает ликвидности» (как
//!   quote/quoter_revert.rs short_pool); иной откат — откат квотера; прочий текст — сбой узла (не откат);
//! - predicted: установленные тики пула в словах битмапа по карте тиков в памяти — их данные читаются в том же
//!   пакете, что slot0 и квотер; same / fits: проверка, что на блоке пакета битмап этих слов тот же, а слова вокруг
//!   свежего тика лежат внутри прочитанных (тогда карта на блоке полная и точная; иначе пул дочитывается обычным
//!   чтением на том же блоке).
//!
//! Кто вызывает: check/hook_batch.rs; tests/bsc_dex_hook_pools_1116_rules.rs (classify на откатах записи узла).
//! От чего зависит: quote/quoter_revert.rs (short_pool_id), pools/ticks.rs (TickMap).
//! Настройки: нет.

use alloy_primitives::{B256, Bytes};

use super::super::pools::ticks::TickMap;
use super::super::quote::quoter_revert;

/// Отказ вызова квотера в пакете.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// UnexpectedRevertBytes(NotEnoughLiquidity(PoolId)) — в пуле не хватает ликвидности на сумму пробы.
    Short(B256),
    /// Иной откат квотера (крючок не дал обмен и т.п.).
    Revert,
    /// Не откат: сбой узла на этом вызове пачки JSON-RPC.
    Fault,
}

/// Метки отката в тексте отказа: node/multicall.rs decode и ответ узла geth (errors.rs from_rpc).
const MARKS: [&str; 2] = ["вызов откатился", "execution reverted"];

/// Что делает: разбор текста отказа вызова квотера (см. шапку). Принимает: текст. Возвращает: Fail. Ошибок нет.
pub fn classify(text: &str) -> Fail {
    let Some(at) = MARKS.iter().filter_map(|m| text.find(m)).min() else {
        return Fail::Fault;
    };
    let data = revert_hex(&text[at..]);
    match data
        .as_deref()
        .and_then(|d| quoter_revert::short_pool_id(&d[..]))
    {
        Some(id) => Fail::Short(id),
        None => Fail::Revert,
    }
}

/// Байты отката: первое «0x…» (hex) после метки. None — данных нет или не hex. Ошибок нет.
fn revert_hex(text: &str) -> Option<Bytes> {
    let start = text.find("0x")?;
    let hex: String = text[start + 2..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    format!("0x{hex}").parse().ok()
}

/// Установленные тики слов [lo, hi] по карте в памяти. None — карта не покрывает эти слова (предсказать нельзя).
/// Ошибок нет.
pub fn predicted(m: &TickMap, (lo, hi): (i16, i16)) -> Option<Vec<i32>> {
    if m.words.0 > lo || m.words.1 < hi {
        return None;
    }
    Some(
        m.ticks
            .keys()
            .copied()
            .filter(|t| (lo..=hi).contains(&TickMap::word_of(*t, m.spacing)))
            .collect(),
    )
}

/// Те же установленные тики (без учёта порядка). Ошибок нет.
pub fn same(a: &[i32], b: &[i32]) -> bool {
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

/// Слова `inner` лежат внутри `outer`. Ошибок нет.
pub fn fits(inner: (i16, i16), outer: (i16, i16)) -> bool {
    outer.0 <= inner.0 && inner.1 <= outer.1
}
