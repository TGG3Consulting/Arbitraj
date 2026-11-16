//! Файл: core/feed_confirm_queue.rs
//! Что делает: решение 367 (задача t163, доработка по ревью Н2) — какие тихие пары спросить у биржи по REST в этом
//! проходе сторожа и в каком порядке делить ведро запросов площадки (venue.quiet_confirm_max_per_s). Для всех бирж.
//! Правила:
//! - ранняя проверка (до того, как котировка признает стакан устаревшим: тишина больше venue.book_stale_ms −
//!   venue.pair_resub_interval_ms) — ТОЛЬКО для пар, которые нужны торговле сейчас: монета с идущей сделкой на площадке,
//!   направление в «держится» или «кандидат» на «Живом экране», направление в очереди расчёта (core/feed_need.rs);
//!   остальные тихие пары — как до правки: после тишины venue.book_stale_ms × venue.pair_dead_mult;
//! - порядок ведра — по «сроку ожидания» пары (order): с какого момента пара ждёт проверки (тишина перешла порог или
//!   истёк venue.quiet_confirm_min_ms прошлой проверки — что позже); у ранних — минус venue.quiet_confirm_min_ms
//!   (приоритет на один срок проверки); при равенстве — порядок снимка;
//! - ведро пусто — остальные пары прохода «отложены ведром» (счётчик для сводки журнала, Stats).
//!
//! Почему молчащая пара не голодает бесконечно (доказательство; поправлено по повторному ревью t163, Н2): пусть пара P
//! молчит (кадров нет) и ждёт. Её order может меняться, только если меняется «нужна торговле» (порог ранней проверки
//! и сдвиг −min_ms), но всегда order(P) ≤ T0 = max(начало тишины + book_stale_ms × pair_dead_mult, next_at) — order
//! обычной проверки: порог ранней (book_stale_ms − pair_resub_interval_ms) не больше book_stale_ms × pair_dead_mult
//! (pair_dead_mult ≥ 1), сдвиг ранней только уменьшает order. T0 не меняется, пока P не спросили и нет кадра. Пару Q
//! спрашивают раньше P, только если order(Q) ≤ order(P) ≤ T0. После проверки Q в момент s её новый order не меньше
//! s − min_ms (обычная — next_at = s + min_ms; ранняя — next_at − min_ms = s; повторная сверка после первого «книга
//! новее» — время того запроса минус min_ms), после кадра Q в момент f — не меньше f − min_ms. Значит, после момента
//! T0 + min_ms ни одна пара не обгонит P; до него — конечное число раз. Ведро даёт не больше quiet_confirm_max_per_s
//! запросов за проход (запас ведра — 1 с), проход — раз в pair_resub_interval_ms: P спросят не позже чем через
//! min_ms + N × pair_resub_interval_ms / quiet_confirm_max_per_s после T0 (N — пар площадки; при интервале < 1 с
//! пополнение за проход меньше quiet_confirm_max_per_s — оценка N / quiet_confirm_max_per_s секунд).
//! Пара с кадрами свежа по кадрам — ей подтверждение не нужно, её сроки двигаются вместе с кадрами.
//! Ёмкость (сколько тихих пар площадка держит подтверждёнными): за проход ведро даёт не больше quiet_confirm_max_per_s
//! запросов (запас ведра — 1 с), проходы — раз в pair_resub_interval_ms, каждая пара — раз в quiet_confirm_min_ms:
//! пар ≤ quiet_confirm_max_per_s × quiet_confirm_min_ms / pair_resub_interval_ms.
//! Кто вызывает: core/feed_watch_logic.rs (decide). От чего зависит: feed_watch_logic.rs (State, Cfg, Action),
//! feed_confirm.rs (CheckSt, Bucket), gateway/feed_health.rs.
//! Настройки: значения приходят в Cfg (читает feed_watch.rs).

use crate::gateway::feed_health::PairHealth;

use super::feed_watch_logic::{Action, Cfg, State};

/// Счётчики сторожа площадки с запуска (сводка журнала показывает прирост с прошлой сводки, feed_watch_log.rs).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Проверок, отложенных пустым ведром.
    pub deferred: u64,
    /// Ранних проверок (пара нужна торговле, решение 367).
    pub early_checks: u64,
    /// Ранних проверок с ответом «книга та же» (стакан подтверждён до отказа котировки).
    pub early_confirmed: u64,
    /// Ранних «книга новее» в первый раз — без переподписки, повторная сверка следующим проходом (кадр мог быть в пути).
    pub early_suspect: u64,
    /// Ранних «книга новее» второй раз подряд без кадра — кадры потеряны, переподписка.
    pub early_lost: u64,
}

/// Пара, которую пора спросить: (соединение, тикер), срок ожидания, ранняя ли проверка (пара нужна торговле: и до
/// порога book_stale_ms × pair_dead_mult, и после него — приоритет в ведре и повторная сверка первого «книга новее»).
pub(super) struct Want {
    pub key: (String, String),
    pub order: i64,
    pub early: bool,
}

/// Порог тишины ранней проверки: venue.book_stale_ms − venue.pair_resub_interval_ms, не меньше 0. Ошибок нет.
fn early_after(c: &Cfg) -> i64 {
    (c.book_stale_ms - c.interval_ms).max(0)
}

/// Пора ли ранняя проверка пары: прогрев после (пере)подписки прошёл и тишина (от последнего кадра или
/// (пере)подписки — что позже) больше early_after. Принимает: пару, сейчас (unix мс), настройки. Ошибок нет.
pub(super) fn due(p: &PairHealth, now: i64, c: &Cfg) -> bool {
    now >= p.since_ms + c.warmup_ms && now - from(p) > early_after(c)
}

/// Начало тишины пары: последний кадр или (пере)подписка — что позже. Ошибок нет.
fn from(p: &PairHealth) -> i64 {
    p.last_ms.unwrap_or(0).max(p.since_ms)
}

/// Пара ждёт проверки (порог тишины уже перейдён вызывающим): None — рано (пауза круга переподписки, срок
/// venue.quiet_confirm_min_ms прошлой проверки не истёк). Принимает: состояние, ключ, пару, сейчас, настройки,
/// venue.quiet_confirm_min_ms, ранняя ли. Ошибок нет.
pub(super) fn wanted(
    st: &State,
    key: &(String, String),
    p: &PairHealth,
    now: i64,
    c: &Cfg,
    min_ms: i64,
    early: bool,
) -> Option<Want> {
    if st.pairs.get(key).is_some_and(|s| now < s.next_at) {
        return None;
    }
    let cs = st.checks.get(key);
    let next_at = cs.map_or(0, |s| s.next_at);
    if now < next_at {
        return None;
    }
    let thr = if early {
        early_after(c)
    } else {
        c.book_stale_ms * c.dead_mult
    };
    // Повторная сверка после первого «книга новее» ранней проверки — с момента того запроса.
    let since = match cs.and_then(|s| s.suspect) {
        Some(sent) => sent,
        None => (from(p) + thr).max(next_at),
    };
    Some(Want {
        key: key.clone(),
        order: if early { since - min_ms } else { since },
        early,
    })
}

/// Раздать ведро прохода: по возрастанию order (равные — в порядке снимка), пока ведро даёт запрос; остальные —
/// «отложены ведром». Принимает: состояние, пары прохода, сейчас, venue.quiet_confirm_min_ms и _max_per_s, выход.
/// Добавляет Action::Check. Ошибок нет.
pub(super) fn serve(
    st: &mut State,
    mut wants: Vec<Want>,
    now: i64,
    min_ms: i64,
    per_s: i64,
    out: &mut Vec<Action>,
) {
    wants.sort_by_key(|w| w.order);
    let n = wants.len();
    for (i, w) in wants.into_iter().enumerate() {
        if !st.bucket.take(now, per_s) {
            st.stats.deferred += (n - i) as u64;
            return;
        }
        let cs = st.checks.entry(w.key.clone()).or_default();
        cs.next_at = now + min_ms;
        cs.asked_ms = now;
        cs.early = w.early;
        if w.early {
            st.stats.early_checks += 1;
        }
        out.push(Action::Check {
            conn: w.key.0,
            ticker: w.key.1,
        });
    }
}
