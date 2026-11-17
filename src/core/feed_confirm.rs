//! Файл: core/feed_confirm.rs
//! Что делает: решение 213 (Тигран, 07.11.2026: «тишина пары ≠ смерть») — чистая логика подтверждения тихой пары по
//! REST: когда спросить биржу (не чаще venue.quiet_confirm_min_ms на пару и venue.quiet_confirm_max_per_s на
//! площадку — ведро запросов), как сравнить версию книги на бирже с версией последнего кадра (verdict) и что сделать
//! по ответу (apply): подтверждена — стакан свежий до следующей проверки, не переподписывать; кадры потеряны —
//! переподписка по решению 202; ошибка — журнал (раз на изменение), вслепую не переподписывать.
//! Сравнение — по номеру/времени книги (Gate) или по содержимому стакана (первые N уровней REST и своего стакана —
//! feed_print.rs; MEXC, Bitget; Тигран 08.11).
//! Дополнение 10.11 (Тигран: согласовано): «кадры потеряны» при том же ответе REST, что и до переподписки, — книга не
//! менялась с переподписки: Unchanged и у бирж без времени изменения книги (feed_rest_ref.rs); действия по
//! Unchanged / Lost / CheckFailed выдаются на каждый ответ с признаком log (смена состояния), журнал с частотой
//! venue.feed_watch_log_ms решает feed_watch_log.rs.
//! t79 (Тигран 10.11: согласовано): у бирж по содержимому такой Unchanged несёт уровни ЭТОГО ответа REST (RestBook) —
//! ядро ставит их стаканом пары (feed_rest_book.rs); Lost, Quiet, ошибка — без стакана.
//! t163 (решение 367): когда и в каком порядке спрашивать — feed_confirm_queue.rs (ранняя проверка только для пар,
//! нужных торговле); ответ отбрасывается, только если кадр пары пришёл ПОСЛЕ отправки запроса (раньше — «кадр моложе
//! book_stale_ms × pair_dead_mult», из-за чего ранний ответ терялся); первое «книга новее» ранней проверки — без
//! переподписки, повторная сверка следующим проходом (кадр мог быть в пути), второе подряд — кадры потеряны.
//! Решение 383 (t169): срок подтверждения — отправка + venue.quiet_confirm_min_ms + запас хода (замер: от начала
//! прошлого прохода до применения ответов, feed_confirm_queue.rs lead) — повторная проверка успевает ответить до срока;
//! срок запоминается (CheckSt.until_ms) — сводка журнала отличает действующее подтверждение от истёкшего.
//! Без ожиданий и ввода-вывода — проверяется тестами (feed_confirm_tests.rs, feed_print_tests.rs).
//! Кто вызывает: core/feed_watch_logic.rs (note_quiet), feed_confirm_queue.rs (Bucket), core/feed_watch.rs (apply).
//! От чего зависит: gateway/feed_health.rs, feed_watch_logic.rs (Action, Cfg, State, start_round), feed_print.rs,
//! feed_rest_ref.rs.
//! Настройки: значения приходят в Cfg.quiet (читает feed_watch.rs).

use std::collections::VecDeque;

use crate::gateway::feed_health::{BookVersion, FeedHealth, PairHealth};

use super::feed_rest_ref::{LostRef, RestBook, RestRef, unchanged_since_resub};
use super::feed_watch_logic::{Action, Cfg, State, start_round};

/// Подтверждение тихой пары: настройки площадки или причина, почему выключено.
#[derive(Debug, Clone, PartialEq)]
pub enum QuietCfg {
    /// venue.quiet_confirm_min_ms (мс) и venue.quiet_confirm_max_per_s (запросов/с).
    Confirm { min_ms: i64, max_per_s: i64 },
    /// Не задана настройка — текст причины.
    Off(String),
}

/// Последнее сказанное в журнал о тихой паре (журнал — только при смене).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Told {
    Quiet,
    Unchanged,
    Lost,
    Failed(String),
    NoConfirm,
}

/// Проверки одной пары.
#[derive(Debug, Default)]
pub(super) struct CheckSt {
    /// Следующая проверка — не раньше (unix мс).
    pub(super) next_at: i64,
    /// t163: когда биржу спросили в последний раз (unix мс): кадр позже — срок проверки сброшен (feed_watch_logic.rs).
    pub(super) asked_ms: i64,
    pub(super) told: Option<Told>,
    /// Решение 367 (ревью Н2) / 383: последняя проверка — пары, нужной торговле (feed_confirm_queue.rs Kind::Need):
    /// первое «книга новее» — повторная сверка без переподписки; счётчики сводки «проверок нужных торговле пар».
    /// Ранние проверки из остатка ведра (Kind::Spare) — как обычные (ревью t169, Н1).
    pub(super) need: bool,
    /// Решение 383: до какого момента (unix мс) действует подтверждение «книга та же» (0 — не действует).
    pub(super) until_ms: i64,
    /// Решение 367 (ревью Н2): первая ранняя проверка сказала «книга новее» — время её запроса (unix мс); повторная
    /// сверка следующим проходом, переподписка — только при втором «новее» без кадра между ними. Кадр — сброс.
    pub(super) suspect: Option<i64>,
    /// Повторное ревью t163, Н1: пара была нужна торговле, когда её тишина перешла порог ранней проверки, — остаётся
    /// нужной, пока книгу не подтвердят (Quiet, Unchanged) или не придёт новый кадр (feed_watch_logic.rs).
    pub(super) sticky: bool,
    /// Дополнение 10.11: ответ REST, давший «кадры потеряны» (feed_rest_ref.rs); тихая или свежая пара — None.
    pub(super) lost_ref: Option<LostRef>,
}

/// Решение 383: секунда, мс — единица из имени настройки venue.quiet_confirm_max_per_s («запросов в секунду»), не
/// порог и не настраиваемое число: окно ведра (Bucket) и период прохода подтверждений (feed_watch.rs).
pub(super) const SECOND_MS: i64 = 1_000;

/// Ведро запросов площадки (решение 383, ревью t169 Н2 — строгое окно): в ЛЮБОМ окне длиной SECOND_MS (по часам
/// прохода сторожа) — не больше venue.quiet_confirm_max_per_s запросов, значит за 10 с — не больше 10 × max_per_s.
/// Хранит моменты выданных запросов последней секунды (их не больше max_per_s).
#[derive(Debug, Default)]
pub(super) struct Bucket {
    sent: VecDeque<i64>,
}

impl Bucket {
    /// Взять один запрос в момент now (unix мс, часы прохода; моменты не убывают). Возвращает: true — можно слать
    /// (за последние SECOND_MS выдано меньше per_s). Ошибок нет.
    pub(super) fn take(&mut self, now: i64, per_s: i64) -> bool {
        while self.sent.front().is_some_and(|&t| t <= now - SECOND_MS) {
            self.sent.pop_front();
        }
        if (self.sent.len() as i64) < per_s {
            self.sent.push_back(now);
            true
        } else {
            false
        }
    }
}

/// Итог сравнения версии книги на бирже с последним кадром пары.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Книга не менялась с нашего последнего кадра — стакан в памяти верный.
    Quiet,
    /// Книга менялась после нашего кадра, но ДО (пере)подписки и с подписки не менялась: кадр биржа пришлёт только
    /// при изменении — переподписка не поможет, а стакана этой версии у нас нет (свежим не считается).
    Unchanged,
    /// Книга менялась после нашего кадра и после (пере)подписки, а кадра нет — кадры потеряны (что именно — текст).
    Lost(String),
    /// Сравнить нельзя (текст причины): биржа не дала сравнимой версии или запрос не прошёл.
    Unknown(String),
}

/// Сравнение. Принимает: пару (версия последнего кадра, время (пере)подписки), версию по REST, время получения ответа
/// (часы бота). Правила по порядку:
/// 1) номер: есть у кадра и у ответа — номер ответа не больше — Quiet, иначе книга новее;
/// 2) время изменения по часам биржи: есть у кадра и у ответа — так же;
/// 3) кадров с подписки не было — книга «новее» (сравнивать не с чем);
/// 4) иначе — Unknown.
///
/// Книга новее: если биржа дала возраст книги (server_ms − changed_ms) и он не меньше времени с (пере)подписки до
/// ответа (по часам бота, без сравнения часов) — Unchanged, иначе Lost.
///
/// По содержимому (Тигран 08.11): адаптер дал отпечатки (v.print) и сравнения по номеру нет (номера нет у кадра или
/// у ответа) — feed_print.rs::by_content: равны — Quiet, иначе Lost. Номер, когда он есть у обоих, решает как раньше
/// (Gate отпечатков не даёт). Ошибок нет.
pub fn verdict(p: &PairHealth, v: &BookVersion, recv_ms: i64) -> Verdict {
    if let Some(prints) = &v.print
        && (p.last_id.is_none() || v.id.is_none())
    {
        return super::feed_print::by_content(prints);
    }
    let why = match (p.last_id, v.id, p.last_changed_ms, v.changed_ms) {
        (Some(ours), Some(theirs), _, _) if theirs <= ours => return Verdict::Quiet,
        (Some(ours), Some(theirs), _, _) => {
            format!("номер книги на бирже {theirs} новее номера последнего кадра {ours}")
        }
        (_, _, Some(ours), Some(theirs)) if theirs <= ours => return Verdict::Quiet,
        (_, _, Some(ours), Some(theirs)) => {
            format!("книга на бирже изменилась в {theirs} мс, последний кадр — книга от {ours} мс")
        }
        _ if p.last_ms.is_none() => "книга на бирже есть, а кадров с подписки не было".into(),
        _ => {
            return Verdict::Unknown(
                "биржа не дала версию, сравнимую с последним кадром (номер или время изменения)"
                    .into(),
            );
        }
    };
    match (v.changed_ms, v.server_ms) {
        (Some(c), Some(s)) if s - c >= recv_ms - p.since_ms => Verdict::Unchanged,
        _ => Verdict::Lost(why),
    }
}

/// Тихая пара без подтверждения (адаптер не умеет или настройка не задана) и без расписания кадров: только журнал.
/// Возвращает: true — пара стала тихой впервые (сказать в журнал). Ошибок нет.
pub(super) fn note_quiet(st: &mut State, key: &(String, String)) -> bool {
    let c = st.checks.entry(key.clone()).or_default();
    let new = c.told != Some(Told::NoConfirm);
    c.told = Some(Told::NoConfirm);
    new
}

/// Ответ биржи на проверку. Принимает: снимок свежести ПОСЛЕ ответа (кадр мог прийти, пока шёл запрос), соединение,
/// тикер, ответ (ошибка — текст), когда запрос отправлен и когда получен ЭТОТ ответ (часы бота), настройки, состояние.
/// Возвращает: действия (подтверждение, переподписка; у каждого ответа — признак log: состояние пары сменилось).
/// Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    h: &FeedHealth,
    conn: &str,
    ticker: &str,
    r: Result<BookVersion, String>,
    sent_ms: i64,
    recv_ms: i64,
    c: &Cfg,
    st: &mut State,
) -> Vec<Action> {
    let mut out = vec![];
    let QuietCfg::Confirm { min_ms, .. } = c.quiet else {
        return out;
    };
    let lead = super::feed_confirm_queue::lead(st, c);
    let Some(p) = h
        .conns
        .iter()
        .find(|k| k.conn == conn)
        .and_then(|k| k.pairs.iter().find(|p| p.ticker == ticker))
    else {
        return out;
    };
    // Пока шёл запрос, пришёл кадр — стакан в памяти новее ответа, пара живая, ответ уже не нужен. t163 (решение 367):
    // по времени отправки, а не по «кадр моложе book_stale_ms × pair_dead_mult» — иначе ранний ответ (due) терялся.
    if p.last_ms.is_some_and(|l| l > sent_ms) {
        return out;
    }
    let rest = r.as_ref().ok().and_then(RestRef::of);
    let v = match r {
        Ok(v) => verdict(p, &v, recv_ms),
        Err(e) => Verdict::Unknown(format!("запрос версии стакана не прошёл: {e}")),
    };
    let key = (conn.to_string(), ticker.to_string());
    let cs = st.checks.entry(key.clone()).or_default();
    // t163: запрос этого ответа отправлен в sent_ms — кадр раньше него к сбросу срока проверки не ведёт.
    cs.asked_ms = cs.asked_ms.max(sent_ms);
    // Дополнение 10.11: тот же ответ REST, что дал «кадры потеряны», а пара с тех пор переподписана — книга не
    // менялась с переподписки (тихая, не мёртвая); иначе запомнить ответ для следующей проверки.
    // t79: уровни этого ответа (у бирж по содержимому) — стакан пары; только здесь, ни при каком другом исходе.
    let mut book = None;
    let v = match v {
        Verdict::Lost(_) if unchanged_since_resub(cs.lost_ref.as_ref(), rest.as_ref(), p) => {
            book = RestBook::of(rest.as_ref(), sent_ms, recv_ms);
            Verdict::Unchanged
        }
        Verdict::Lost(why) => {
            cs.lost_ref = rest.map(|rest| LostRef {
                rest,
                at_ms: recv_ms,
            });
            Verdict::Lost(why)
        }
        Verdict::Quiet => {
            cs.lost_ref = None;
            Verdict::Quiet
        }
        other => other,
    };
    // Решение 367 (ревью Н2): ранняя проверка сказала «книга новее» впервые — книга могла измениться за миг до запроса,
    // а кадр с изменением ещё в пути (пуш у бирж — раз в 100–1000 мс). Не переподписывать: повторная сверка (срок — ниже);
    // кадр придёт — сверка снимается (feed_watch_logic.rs, сброс по кадру). Второе «новее» без кадра — кадры потеряны,
    // как всегда. Обычная (не нужная торговле) проверка — как до правки.
    // Повторное ревью t163, Б1: книга на бирже новее — старое подтверждение тихой пары снимается сразу (Unconfirm),
    // иначе котировка до его срока шла бы по книге, про которую уже известно, что она изменилась.
    // Ревью t169, Н1: повторная сверка — не раньше venue.pair_resub_interval_ms после первого запроса (по времени, как
    // до решения 383, когда проход шёл раз в этот интервал), а не «следующим проходом» (теперь раз в секунду); только
    // у нужных торговле (решение 367), ранние из остатка ведра — как обычные.
    if matches!(v, Verdict::Lost(_)) && cs.need && cs.suspect.is_none() {
        cs.suspect = Some(sent_ms);
        cs.next_at = sent_ms + c.interval_ms;
        cs.until_ms = 0;
        st.stats.early_suspect += 1;
        out.push(Action::Unconfirm {
            conn: key.0.clone(),
            ticker: key.1.clone(),
        });
        return out;
    }
    let need = cs.need;
    // Повторное ревью t163, Н4: ошибка REST (Unknown) сверку не снимает — иначе чередование «новее» / ошибка
    // откладывало бы переподписку бесконечно; снимают только ответы по существу.
    if !matches!(v, Verdict::Unknown(_)) {
        cs.suspect = None;
    }
    match &v {
        Verdict::Quiet if need => st.stats.early_confirmed += 1,
        Verdict::Lost(_) if need => st.stats.early_lost += 1,
        _ => {}
    }
    // Повторное ревью t163, Н1: книга подтверждена или принята как есть — приоритет «нужной» больше не держится.
    if matches!(v, Verdict::Quiet | Verdict::Unchanged) {
        cs.sticky = false;
    }
    let told = match &v {
        Verdict::Quiet => Told::Quiet,
        Verdict::Unchanged => Told::Unchanged,
        Verdict::Lost(_) => Told::Lost,
        Verdict::Unknown(w) => Told::Failed(w.clone()),
    };
    let changed = cs.told.as_ref() != Some(&told);
    cs.told = Some(told);
    // Решение 383: срок подтверждения — только у «книга та же»; Unchanged и Lost снимают подтверждение (Б1), ошибка —
    // не трогает (адаптер держит прежний срок).
    let until_ms = sent_ms + min_ms + lead;
    match &v {
        Verdict::Quiet => cs.until_ms = until_ms,
        Verdict::Unchanged | Verdict::Lost(_) => cs.until_ms = 0,
        Verdict::Unknown(_) => {}
    }
    let (conn, ticker) = (key.0.clone(), key.1.clone());
    match v {
        Verdict::Quiet => {
            st.pairs.remove(&key);
            // Свежий до ответа следующей проверки: она назначена на отправку + venue.quiet_confirm_min_ms, уходит на
            // первом проходе после этого и отвечает — всё это укладывается в запас хода (решение 383; до него —
            // venue.pair_resub_interval_ms без времени ответа, дыра на время запроса). Отсчёт — от отправки запроса
            // (книга не менялась как минимум до неё).
            out.push(Action::Confirmed {
                conn,
                ticker,
                until_ms,
                log: changed,
            });
        }
        Verdict::Unchanged => {
            st.pairs.remove(&key);
            out.push(Action::Unchanged {
                conn,
                ticker,
                book,
                log: changed,
            });
        }
        Verdict::Lost(why) => {
            out.push(Action::Lost {
                conn: conn.clone(),
                ticker: ticker.clone(),
                why,
                log: changed,
            });
            start_round(st, &key, recv_ms, c, &mut out);
        }
        Verdict::Unknown(why) => {
            out.push(Action::CheckFailed {
                conn: conn.clone(),
                ticker: ticker.clone(),
                why,
                log: changed,
            });
            // Кадры по документации идут по расписанию — тишина и без REST значит мёртвую подписку (решение 202).
            if h.scheduled {
                start_round(st, &key, recv_ms, c, &mut out);
            }
        }
    }
    out
}
