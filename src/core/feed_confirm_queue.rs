//! Файл: core/feed_confirm_queue.rs
//! Что делает: решения 367 и 383 — какие тихие пары спросить у биржи по REST в этом проходе сторожа и в каком порядке
//! делить ведро запросов площадки (venue.quiet_confirm_max_per_s). Для всех бирж.
//! Правила:
//! - запас хода (lead) — сколько может пройти от порога ранней проверки до применения ответа: от начала прошлого
//!   прохода до применения ответов этого (замер feed_watch.rs, State::note_answers; решение 383); замера ещё нет —
//!   venue.pair_resub_interval_ms (как в решении 367);
//! - ранняя проверка — когда тишина больше venue.book_stale_ms − lead (до того, как котировка признает стакан
//!   устаревшим): у пар, нужных торговле (монета с идущей сделкой на площадке, «держится» или «кандидат» на «Живом
//!   экране», очередь расчёта — core/feed_need.rs), — в приоритете; решение 383: у остальных пар — из ОСТАТКА ведра
//!   (после всех прочих, «отложены ведром» не считаются: через порог мёртвой пары они станут обычными);
//! - обычная проверка — тишина больше venue.book_stale_ms × venue.pair_dead_mult;
//! - порядок ведра — ярус (нужные и обычные; затем ранние из остатка), внутри — «срок ожидания» (order): с какого
//!   момента пара ждёт проверки (тишина перешла порог или истёк venue.quiet_confirm_min_ms прошлой проверки — что
//!   позже); у нужных — минус venue.quiet_confirm_min_ms (приоритет на один срок проверки); при равенстве — порядок
//!   снимка.
//!
//! Почему молчащая пара не голодает бесконечно: пусть пара P молчит и ждёт обычной или нужной проверки. Всегда order(P)
//! ≤ T0 = max(начало тишины + book_stale_ms × pair_dead_mult, next_at): порог ранней (book_stale_ms − lead) не больше
//! book_stale_ms × pair_dead_mult (pair_dead_mult ≥ 1), сдвиг нужных только уменьшает order. T0 не меняется, пока P не
//! спросили и нет кадра. Ранние из остатка P не обгоняют (ярус ниже). Пару Q спрашивают раньше P, только если
//! order(Q) ≤ order(P) ≤ T0; после проверки Q в момент s её новый order не меньше s − min_ms (повторная сверка после
//! первого «книга новее» — время того запроса минус min_ms), после кадра Q в момент f — не меньше f − min_ms. Значит,
//! после T0 + min_ms ни одна пара не обгонит P; до него — конечное число раз. Пара из остатка ждёт не дольше, чем до
//! порога мёртвой пары, — дальше она обычная.
//!
//! Ёмкость (решение 383; факт кода): ведро — venue.quiet_confirm_max_per_s запросов в секунду (запас — 1 с, пополнение
//! по времени), проход подтверждений — раз в секунду (feed_watch.rs), каждая пара — не чаще venue.quiet_confirm_min_ms:
//! подтверждёнными держится до quiet_confirm_max_per_s × quiet_confirm_min_ms / 1000 тихих пар (минус запросы нужных и
//! ранних). До решения 383 проход был раз в venue.pair_resub_interval_ms + время прохода, а запас ведра — 1 с: за
//! проход не больше quiet_confirm_max_per_s запросов, ёмкость — quiet_confirm_max_per_s × quiet_confirm_min_ms /
//! (pair_resub_interval_ms + время прохода).
//! Кто вызывает: core/feed_watch_logic.rs (decide), feed_confirm.rs (lead). От чего зависит: feed_watch_logic.rs
//! (State, Cfg, Action), feed_confirm.rs (CheckSt, Bucket), gateway/feed_health.rs.
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
    /// Решение 383: ранних проверок из остатка ведра (пара торговле не нужна).
    pub spare_checks: u64,
    /// Ранних проверок с ответом «книга та же» (стакан подтверждён до отказа котировки).
    pub early_confirmed: u64,
    /// Ранних «книга новее» в первый раз — без переподписки, повторная сверка следующим проходом (кадр мог быть в пути).
    pub early_suspect: u64,
    /// Ранних «книга новее» второй раз подряд без кадра — кадры потеряны, переподписка.
    pub early_lost: u64,
}

impl State {
    /// Решение 383: замер запаса хода прохода с проверками. Принимает: начало прошлого прохода и момент, когда ответы
    /// этого прохода применены (unix мс). Ошибок нет.
    pub fn note_answers(&mut self, prev_start_ms: i64, applied_ms: i64) {
        self.lead_ms = Some((applied_ms - prev_start_ms).max(0));
    }
}

/// Вид проверки пары в этом проходе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Kind {
    /// Пара нужна торговле: ранний порог, приоритет в ведре (решение 367).
    Need,
    /// Решение 383: пара не нужна торговле, тишина между ранним порогом и порогом мёртвой пары — из остатка ведра.
    Spare,
    /// Обычная: тишина больше book_stale_ms × pair_dead_mult.
    Plain,
}

/// Пара, которую пора спросить: (соединение, тикер), срок ожидания, ранняя ли проверка (до порога мёртвой пары или
/// нужная — повторная сверка первого «книга новее» без переподписки), нужна ли торговле (приоритет, счётчики).
pub(super) struct Want {
    pub key: (String, String),
    pub order: i64,
    pub early: bool,
    pub need: bool,
}

/// Решение 383: запас хода — от начала прошлого прохода до применения ответов (замер, State.lead_ms); замера нет —
/// venue.pair_resub_interval_ms. Ошибок нет.
pub(super) fn lead(st: &State, c: &Cfg) -> i64 {
    st.lead_ms.unwrap_or(c.interval_ms)
}

/// Порог тишины ранней проверки: venue.book_stale_ms − запас хода, не меньше 0. Ошибок нет.
fn early_after(c: &Cfg, lead: i64) -> i64 {
    (c.book_stale_ms - lead).max(0)
}

/// Пора ли ранняя проверка пары: прогрев после (пере)подписки прошёл и тишина (от последнего кадра или
/// (пере)подписки — что позже) больше early_after. Принимает: пару, сейчас (unix мс), настройки, запас хода (lead).
/// Ошибок нет.
pub(super) fn due(p: &PairHealth, now: i64, c: &Cfg, lead: i64) -> bool {
    now >= p.since_ms + c.warmup_ms && now - from(p) > early_after(c, lead)
}

/// Начало тишины пары: последний кадр или (пере)подписка — что позже. Ошибок нет.
fn from(p: &PairHealth) -> i64 {
    p.last_ms.unwrap_or(0).max(p.since_ms)
}

/// Пара ждёт проверки (порог тишины уже перейдён вызывающим): None — рано (пауза круга переподписки, срок
/// venue.quiet_confirm_min_ms прошлой проверки не истёк). Принимает: состояние, ключ, пару, сейчас, настройки,
/// venue.quiet_confirm_min_ms, вид проверки. Ошибок нет.
pub(super) fn wanted(
    st: &State,
    key: &(String, String),
    p: &PairHealth,
    now: i64,
    c: &Cfg,
    min_ms: i64,
    kind: Kind,
) -> Option<Want> {
    if st.pairs.get(key).is_some_and(|s| now < s.next_at) {
        return None;
    }
    let cs = st.checks.get(key);
    let next_at = cs.map_or(0, |s| s.next_at);
    if now < next_at {
        return None;
    }
    let (early, need) = (kind != Kind::Plain, kind == Kind::Need);
    let thr = if early {
        early_after(c, lead(st, c))
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
        order: if need { since - min_ms } else { since },
        early,
        need,
    })
}

/// Раздать ведро прохода: ярус (ранние из остатка — последними), внутри — по возрастанию order (равные — в порядке
/// снимка), пока ведро даёт запрос; остальные нужные и обычные — «отложены ведром». Принимает: состояние, пары
/// прохода, сейчас, venue.quiet_confirm_min_ms и _max_per_s, выход. Добавляет Action::Check. Ошибок нет.
pub(super) fn serve(
    st: &mut State,
    mut wants: Vec<Want>,
    now: i64,
    min_ms: i64,
    per_s: i64,
    out: &mut Vec<Action>,
) {
    let spare = |w: &Want| w.early && !w.need;
    wants.sort_by_key(|w| (spare(w), w.order));
    let main = wants.iter().filter(|w| !spare(w)).count();
    for (i, w) in wants.into_iter().enumerate() {
        if !st.bucket.take(now, per_s) {
            st.stats.deferred += main.saturating_sub(i) as u64;
            return;
        }
        let cs = st.checks.entry(w.key.clone()).or_default();
        cs.next_at = now + min_ms;
        cs.asked_ms = now;
        cs.early = w.early;
        cs.need = w.need;
        if w.need {
            st.stats.early_checks += 1;
        } else if w.early {
            st.stats.spare_checks += 1;
        }
        out.push(Action::Check {
            conn: w.key.0,
            ticker: w.key.1,
        });
    }
}
