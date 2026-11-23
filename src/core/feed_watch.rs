//! Файл: core/feed_watch.rs
//! Что делает: решение 202 (Тигран, 06.11.2026: «переподписка не биржи, а именно пачки») — сторож пар биржи: раз в
//! venue.pair_resub_interval_ms берёт у адаптера свежесть соединений (контракт feed_health), решает по чистой логике
//! (feed_watch_logic.rs) и выполняет действия (feed_watch_act.rs): переподписать одну пару, переподключить одно
//! соединение (только если по его парам нет идущей сделки, core/feed_hold.rs), журнал и Telegram.
//! Решение 213 (Тигран, 07.11.2026: «тишина пары ≠ смерть»): молчащую пару сторож сначала подтверждает у биржи по REST
//! (контракт book_version; запросы прохода — одновременно, их число ограничивает ведро venue.quiet_confirm_max_per_s),
//! ответ сравнивает со СВЕЖИМ снимком feed_health (кадр мог прийти, пока шёл запрос) — feed_confirm.rs::apply;
//! подтверждённая пара: book_confirmed адаптеру (стакан свежий в котировке до следующей проверки).
//! Решение 367 (t163, ревью Н2): перед проходом — пары, нужные торговле (feed_need.rs): их подтверждают заранее и
//! первыми в ведре (feed_confirm_queue.rs).
//! Решение 383 (t169): при подтверждении тихих пар (адаптер умеет, venue.quiet_confirm_* заданы) проход — раз в секунду
//! (единица venue.quiet_confirm_max_per_s; не реже venue.pair_resub_interval_ms), иначе — раз в
//! venue.pair_resub_interval_ms; период — от начала до начала прохода. Раньше проход шёл раз в
//! venue.pair_resub_interval_ms ПОСЛЕ работы прохода, а ведро держит запас на 1 с — за проход уходило не больше
//! venue.quiet_confirm_max_per_s запросов, т. е. в секунду в ~3 раза меньше настройки. Сроки переподписки, мёртвой пары,
//! переподключения считаются по времени (feed_watch_logic.rs) — от частоты прохода не зависят. После проверок —
//! замер запаса хода (State::note_answers): от начала прошлого прохода до применения ответов (ревью t169, Б1: пауза
//! сторожа сбрасывает замер). Запросы подтверждения уходят сразу после решения прохода, прочие действия — после ответов
//! (ревью t169, Н2: строгое окно ведра по моментам отправки).
//! Сторож работает у любой площадки, чей адаптер отдаёт feed_health (поле scheduled больше его не выключает).
//! Не задана любая настройка решения 202 — сторож площадки выключен (одна запись в журнал), ждёт изменения настроек;
//! не задана настройка подтверждения — подтверждения нет (причина — в журнале вместе с тихими парами).
//! Дополнение 10.11 (Тигран: согласовано): журнал видимости (feed_watch_log.rs) — строка «Сторож пар работает» при
//! включении, сводка прохода не чаще venue.feed_watch_log_ms, строки решений по паре с той же частотой.
//! Кто вызывает: core/engine.rs (задача на каждую биржу).
//! От чего зависит: feed_watch_logic.rs, feed_confirm.rs, feed_watch_act.rs, feed_watch_log.rs, feed_hold.rs,
//! venue_ticker.rs, feed_need.rs, engine.rs, gateway (feed_health, book_version), settings.
//! Настройки: venue.book_stale_ms, venue.pair_dead_mult, feed.stale_after_ms, venue.feed_warmup_ms,
//! venue.pair_resub_attempts, venue.pair_resub_interval_ms, venue.pair_resub_pause_ms, venue.conn_dead_pct,
//! venue.conn_reconnect_min_ms, venue.conn_reconnect_max_per_hour; решение 213 — venue.quiet_confirm_min_ms,
//! venue.quiet_confirm_max_per_s (по площадке); журнал — venue.feed_watch_log_ms (по площадке, необязательная).

use std::collections::HashMap;
use std::time::Duration;

use crate::core::types::VenueId;
use crate::gateway::contract::Venue;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::feed_confirm::QuietCfg;
use super::feed_watch_act::{act, log};
use super::feed_watch_log::LogGate;
use super::feed_watch_logic::{self as logic, Action, Cfg, State};

/// Настройки сторожа площадки. Ошибка — текст: какая не задана (решение 202). Настройки подтверждения (решение 213)
/// не обязательны: не заданы — QuietCfg::Off с причиной.
fn cfg(core: &Core, venue: &VenueId) -> Result<Cfg, String> {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let s = &core.inner.deps.settings;
    let i = |n: &str| s.integer(n, at).map_err(|e| e.message);
    let quiet = match (
        i("venue.quiet_confirm_min_ms"),
        i("venue.quiet_confirm_max_per_s"),
    ) {
        (Ok(min_ms), Ok(per_s)) => QuietCfg::Confirm {
            min_ms: min_ms as i64,
            max_per_s: per_s as i64,
        },
        (Err(e), _) | (_, Err(e)) => QuietCfg::Off(e),
    };
    Ok(Cfg {
        book_stale_ms: i("venue.book_stale_ms")? as i64,
        dead_mult: i("venue.pair_dead_mult")? as i64,
        live_ms: i("feed.stale_after_ms")? as i64,
        warmup_ms: i("venue.feed_warmup_ms")? as i64,
        attempts: i("venue.pair_resub_attempts")? as u32,
        interval_ms: i("venue.pair_resub_interval_ms")? as i64,
        pause_ms: i("venue.pair_resub_pause_ms")? as i64,
        dead_pct: i("venue.conn_dead_pct")? as i64,
        reconnect_min_ms: i("venue.conn_reconnect_min_ms")? as i64,
        reconnect_max_per_hour: i("venue.conn_reconnect_max_per_hour")? as usize,
        quiet,
    })
}

/// Дополнение 10.11: частота журнала сторожа площадки (venue.feed_watch_log_ms). None — не задана. Ошибок нет.
fn log_every(core: &Core, venue: &VenueId) -> Option<i64> {
    let at = At {
        venue: Some(&venue.0),
        coin: None,
    };
    let s = &core.inner.deps.settings;
    s.integer("venue.feed_watch_log_ms", at)
        .ok()
        .map(|v| v as i64)
}

/// Решение 383: период прохода (мс, от начала до начала): подтверждение тихих пар включено — секунда, но не реже
/// venue.pair_resub_interval_ms; иначе — venue.pair_resub_interval_ms. Не меньше 1. Ошибок нет.
fn tick_ms(h: &crate::gateway::feed_health::FeedHealth, c: &Cfg) -> i64 {
    let confirm = h.confirm && matches!(c.quiet, QuietCfg::Confirm { .. });
    let t = if confirm {
        // SECOND_MS — единица из имени настройки venue.quiet_confirm_max_per_s («в секунду»), не порог (feed_confirm.rs).
        c.interval_ms.min(super::feed_confirm::SECOND_MS)
    } else {
        c.interval_ms
    };
    t.max(1)
}

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Задача сторожа одной биржи. Работает, пока жива система. Ошибок не возвращает.
pub async fn run(core: Core, venue: VenueId) {
    let mut st = State::default();
    let mut lg = LogGate::default();
    let mut told: Option<String> = None;
    let mut watch = core.inner.deps.watch(&["venue.", "feed."]);
    // Решение 383: начало прошлого прохода (замер запаса хода).
    let mut prev_start: Option<i64> = None;
    loop {
        let c = match cfg(&core, &venue) {
            Ok(c) => c,
            Err(why) => {
                if told.as_deref() != Some(why.as_str()) {
                    let m = format!("Сторож пар выключен: {why}");
                    log(&core, &venue, Level::Info, m).await;
                    told = Some(why);
                }
                lg.off();
                // Ревью t169, Б1: пауза сторожа — замер запаса хода начинается заново.
                prev_start = None;
                if watch.next().await.is_none() {
                    return;
                }
                continue;
            }
        };
        told = None;
        let Ok(v) = core.inner.deps.gw.get(&venue).await else {
            prev_start = None;
            tokio::time::sleep(Duration::from_millis(c.interval_ms.max(1) as u64)).await;
            continue;
        };
        let Some(h) = v.feed_health() else { return };
        lg.every = log_every(&core, &venue);
        if let Some(m) = lg.announce(&h, &c) {
            log(&core, &venue, Level::Info, m).await;
        }
        let deals: HashMap<String, Vec<String>> = super::feed_hold::holders(&core, &venue)
            .into_iter()
            .fold(HashMap::new(), |mut m, (id, a)| {
                // Решение 211 (ревью В4): здоровье потока площадки — по её тикеру монеты, не по показному имени.
                let t = super::venue_ticker::ticker_on(&core, &a, &venue);
                m.entry(t).or_default().push(id);
                m
            });
        // Решение 367 (ревью Н2): пары, нужные торговле, — ранняя проверка и приоритет в ведре (feed_need.rs).
        st.needed = super::feed_need::needed(&core, &venue);
        // Ревью t169, Н2: часы ведра — момент решения прохода (start; следующий — не раньше start + период прохода);
        // запросы подтверждения уходят сразу после решения, до прочих действий прохода (журнал, переподписки), — момент
        // отправки отстаёт от start только на время решения, и окно ведра (не больше venue.quiet_confirm_max_per_s за
        // любую секунду) держится и по моментам отправки с этой точностью.
        let start = now_ms();
        let (mut checks, mut other) = (vec![], vec![]);
        for a in logic::decide(&h, start, &c, &deals, &mut st) {
            match a {
                Action::Check { conn, ticker } => checks.push((conn, ticker)),
                a => other.push(a),
            }
        }
        let asked = !checks.is_empty();
        confirm(&core, &venue, v.as_ref(), checks, &c, &mut st, &mut lg).await;
        for a in other {
            act(&core, &venue, v.as_ref(), a, &c, &mut lg).await;
        }
        // Решение 383: запас хода — от начала прошлого прохода до применения ответов этого (первый проход замера не
        // даёт — до него venue.pair_resub_interval_ms).
        if let (true, Some(p)) = (asked, prev_start) {
            st.note_answers(p, now_ms());
        }
        prev_start = Some(start);
        if let Some(m) = lg.summary(&h, now_ms(), &c, &st) {
            log(&core, &venue, Level::Info, m).await;
        }
        let rest = tick_ms(&h, &c) - (now_ms() - start);
        tokio::time::sleep(Duration::from_millis(rest.max(0) as u64)).await;
    }
}

/// Решение 213: проверки тихих пар прохода — запросы book_version одновременно (их число уже ограничено ведром),
/// ответы — по свежему снимку feed_health. Тикер — площадки (PairHealth.ticker, решение 211). Ошибок нет: сбой
/// запроса — действие CheckFailed (журнал).
async fn confirm(
    core: &Core,
    venue: &VenueId,
    v: &dyn Venue,
    checks: Vec<(String, String)>,
    c: &Cfg,
    st: &mut State,
    lg: &mut LogGate,
) {
    if checks.is_empty() {
        return;
    }
    // t79: время получения — у каждого ответа своё (стакан из ответа REST свеж от него, а не от последнего ответа
    // прохода).
    let (tx, mut rx) = tokio::sync::mpsc::channel(checks.len());
    let requests = checks.into_iter().map(|(conn, ticker)| async move {
        let sent = now_ms();
        let result = v.book_version(&ticker).await.map_err(|e| e.message);
        (conn, ticker, result, sent, now_ms())
    });
    let producer = deliver(requests, tx);
    let consume = async {
        let mut first = None;
        while let Some((conn, ticker, r, sent, recv)) = rx.recv().await {
            lg.rest_answer(recv - *first.get_or_insert(recv), now_ms() - recv);
            let Some(h) = v.feed_health_for(Some((&conn, &ticker))) else {
                continue;
            };
            for a in super::feed_confirm::apply(&h, &conn, &ticker, r, sent, recv, c, st) {
                act(core, venue, v, a, c, lg).await;
            }
        }
    };
    tokio::join!(producer, consume);
}

/// Запускает все проверки прохода. Получение и отметка времени следующего ответа продолжаются
/// во время записи в журнал или восстановления предыдущей пары. Канал ограничен размером прохода.
async fn deliver<T>(
    requests: impl IntoIterator<Item = impl Future<Output = T>>,
    tx: tokio::sync::mpsc::Sender<T>,
) {
    use futures_util::stream::{FuturesUnordered, StreamExt};
    let mut pending: FuturesUnordered<_> = requests.into_iter().collect();
    while let Some(answer) = pending.next().await {
        if tx.send(answer).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod delivery_tests {
    use futures_util::FutureExt;
    use tokio::sync::{mpsc, oneshot};

    /// Контрольный порядок завершения. При общем барьере результат FAST в этой точке недоступен.
    #[tokio::test]
    async fn fast_book_reaches_consumer_while_slow_request_is_pending() {
        let (release, slow) = oneshot::channel();
        let (tx, mut rx) = mpsc::channel(2);
        let inputs = [
            async {
                slow.await.unwrap();
                "slow"
            }
            .boxed(),
            async { "fast" }.boxed(),
        ];
        let mut produce = Box::pin(super::deliver(inputs, tx));
        assert!(produce.as_mut().now_or_never().is_none());
        assert_eq!(rx.try_recv().unwrap(), "fast");
        release.send(()).unwrap();
        produce.await;
        assert_eq!(rx.recv().await, Some("slow"));
        assert_eq!(rx.recv().await, None);
    }
}
