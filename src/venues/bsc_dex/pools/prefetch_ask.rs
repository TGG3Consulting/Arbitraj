//! Файл: venues/bsc_dex/pools/prefetch_ask.rs
//! Что делает: ревью 325 (Н5) — что читать заранее одним обращением (Ask) и отбор вызовов обращения (pick) так, чтобы
//! чтение заранее не прибавляло обращений к узлу при любом dex.load_batch. Ask: useful — вызовы, которые код чтения
//! делает сам ПООДИНОЧКЕ (call::view): каждый такой вызов, оказавшийся в книге, снимает ровно одно обращение кода
//! чтения (тот же блок, те же байты — node/prefetch.rs); extra — вспомогательные группы, которых код чтения не делает
//! (дерево тиков Algebra Integral: второй слой, листы). pick: все ещё не спрошенные нужные + вспомогательные группы по
//! порядку, пока число частей обращения (по dex.load_batch вызовов, node/call.rs try_chunks) не больше числа нужных;
//! нужных нет — обращения нет. Итог одного обращения заранее: частей ≤ нужных ≤ снятых обращений кода чтения.
//! Так размечены Algebra Integral (prefetch_integral.rs) и KyberSwap Elastic (prefetch_plan.rs). У прочих видов план —
//! From<Vec<Call>> (всё в useful, как до ревью 325): отбор их не меняет.
//! Кто вызывает: pools/prefetch_read.rs (pick), pools/prefetch_plan.rs и prefetch_integral.rs (Ask).
//! От чего зависит: node/prefetch.rs (Book, Call).
//! Настроек нет (размер части — dex.load_batch, передаёт вызывающий).

use super::super::node::prefetch::{Book, Call};

/// Что читать заранее одним обращением.
#[derive(Debug, Default)]
pub struct Ask {
    /// Вызовы, которые код чтения делает сам (каждый, попавший в книгу, снимает его обращение).
    pub useful: Vec<Call>,
    /// Вспомогательные группы по порядку (код чтения их не делает; нужны, чтобы построить следующий план).
    pub extra: Vec<Vec<Call>>,
}

impl From<Vec<Call>> for Ask {
    /// Всё — нужное коду чтения, вспомогательного нет. Ошибок нет.
    fn from(useful: Vec<Call>) -> Ask {
        Ask {
            useful,
            extra: Vec::new(),
        }
    }
}

/// Вызовы одного обращения заранее (см. шапку). Принимает: книгу (что уже спрошено), план, размер части (> 0).
/// Возвращает: вызовы без повторов и без уже спрошенных; пусто — нужных нет. Ошибок нет.
pub fn pick(book: &Book, ask: Ask, part: usize) -> Vec<Call> {
    let mut out = book.fresh(ask.useful);
    let useful = out.len();
    if useful == 0 || part == 0 {
        return Vec::new();
    }
    for group in ask.extra {
        let mut next = out.clone();
        next.extend(group);
        let next = book.fresh(next);
        if next.len().div_ceil(part) > useful {
            break;
        }
        out = next;
    }
    out
}
