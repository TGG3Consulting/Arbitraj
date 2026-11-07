//! Файл: core/feed_confirm.rs
//! Что делает: решение 213 (Тигран, 07.11.2026: «тишина пары ≠ смерть») — чистая логика подтверждения тихой пары по
//! REST: когда спросить биржу (не чаще venue.quiet_confirm_min_ms на пару и venue.quiet_confirm_max_per_s на
//! площадку — ведро запросов), как сравнить версию книги на бирже с версией последнего кадра (verdict) и что сделать
//! по ответу (apply): подтверждена — стакан свежий до следующей проверки, не переподписывать; кадры потеряны —
//! переподписка по решению 202; ошибка — журнал (раз на изменение), вслепую не переподписывать.
//! Без ожиданий и ввода-вывода — проверяется тестами (feed_watch_tests.rs).
//! Кто вызывает: core/feed_watch_logic.rs (want, note_quiet), core/feed_watch.rs (apply).
//! От чего зависит: gateway/feed_health.rs, feed_watch_logic.rs (Action, Cfg, State, start_round).
//! Настройки: значения приходят в Cfg.quiet (читает feed_watch.rs).

use crate::gateway::feed_health::{BookVersion, FeedHealth, PairHealth};

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
    pub(super) told: Option<Told>,
}

/// Ведро запросов площадки: 1 запрос = 1000 долей, пополнение max_per_s долей за мс, запас — на 1 с.
#[derive(Debug, Default)]
pub(super) struct Bucket {
    milli: i64,
    at: Option<i64>,
}

impl Bucket {
    /// Взять один запрос. Возвращает: true — можно слать. Ошибок нет.
    fn take(&mut self, now: i64, per_s: i64) -> bool {
        let cap = per_s * 1000;
        self.milli = match self.at {
            None => cap,
            Some(a) => (self.milli + per_s * (now - a).max(0)).min(cap),
        };
        self.at = Some(now);
        if self.milli >= 1000 {
            self.milli -= 1000;
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
/// ответа (по часам бота, без сравнения часов) — Unchanged, иначе Lost. Ошибок нет.
pub fn verdict(p: &PairHealth, v: &BookVersion, recv_ms: i64) -> Verdict {
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

/// Тихая пара (решение 202: мёртвая по порогу) в режиме подтверждения: пора ли спросить биржу. Пара в паузе после
/// круга переподписки — ждёт паузу; проверка не раньше next_at; ведро площадки пусто — в следующий проход.
/// Принимает: состояние, (соединение, тикер), сейчас, настройки подтверждения. Добавляет Action::Check. Ошибок нет.
pub(super) fn want(
    st: &mut State,
    key: &(String, String),
    now: i64,
    min_ms: i64,
    per_s: i64,
    out: &mut Vec<Action>,
) {
    if st.pairs.get(key).is_some_and(|s| now < s.next_at) {
        return;
    }
    if st.checks.get(key).is_some_and(|c| now < c.next_at) {
        return;
    }
    if !st.bucket.take(now, per_s) {
        return;
    }
    st.checks.entry(key.clone()).or_default().next_at = now + min_ms;
    out.push(Action::Check {
        conn: key.0.clone(),
        ticker: key.1.clone(),
    });
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
/// тикер, ответ (ошибка — текст), когда запрос отправлен и когда получен ответ (часы бота), настройки, состояние.
/// Возвращает: действия (подтверждение, переподписка, записи журнала — только при смене состояния). Ошибок нет.
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
    let Some(p) = h
        .conns
        .iter()
        .find(|k| k.conn == conn)
        .and_then(|k| k.pairs.iter().find(|p| p.ticker == ticker))
    else {
        return out;
    };
    // Пока шёл запрос, пришёл свежий кадр — пара живая, ответ уже не нужен.
    if p.last_ms
        .is_some_and(|l| recv_ms - l <= c.book_stale_ms * c.dead_mult)
    {
        return out;
    }
    let v = match r {
        Ok(v) => verdict(p, &v, recv_ms),
        Err(e) => Verdict::Unknown(format!("запрос версии стакана не прошёл: {e}")),
    };
    let key = (conn.to_string(), ticker.to_string());
    let told = match &v {
        Verdict::Quiet => Told::Quiet,
        Verdict::Unchanged => Told::Unchanged,
        Verdict::Lost(_) => Told::Lost,
        Verdict::Unknown(w) => Told::Failed(w.clone()),
    };
    let cs = st.checks.entry(key.clone()).or_default();
    let changed = cs.told.as_ref() != Some(&told);
    cs.told = Some(told);
    let (conn, ticker) = (key.0.clone(), key.1.clone());
    match v {
        Verdict::Quiet => {
            st.pairs.remove(&key);
            // Свежий до следующей проверки: она назначена на отправку + venue.quiet_confirm_min_ms и делается на
            // проходе сторожа (раз в venue.pair_resub_interval_ms). Отсчёт — от отправки запроса (книга не менялась
            // как минимум до неё).
            let until_ms = sent_ms + min_ms + c.interval_ms;
            out.push(Action::Confirmed {
                conn,
                ticker,
                until_ms,
                log: changed,
            });
        }
        Verdict::Unchanged => {
            st.pairs.remove(&key);
            if changed {
                out.push(Action::Unchanged { conn, ticker });
            }
        }
        Verdict::Lost(why) => {
            if changed {
                out.push(Action::Lost {
                    conn: conn.clone(),
                    ticker: ticker.clone(),
                    why,
                });
            }
            start_round(st, &key, recv_ms, c, &mut out);
        }
        Verdict::Unknown(why) => {
            if changed {
                out.push(Action::CheckFailed {
                    conn: conn.clone(),
                    ticker: ticker.clone(),
                    why,
                });
            }
            // Кадры по документации идут по расписанию — тишина и без REST значит мёртвую подписку (решение 202).
            if h.scheduled {
                start_round(st, &key, recv_ms, c, &mut out);
            }
        }
    }
    out
}
