//! Файл: core/feed_watch_act.rs
//! Что делает: решения 202 и 213 — выполнение действий сторожа пар (feed_watch_action.rs): команда адаптеру
//! (resubscribe_pair, reconnect_conn, book_confirmed), журнал, Telegram. Журнал решения 202 — на каждое действие
//! (попытка — первая в круге), Telegram — «переподключить нельзя: идёт сделка» и «соединение не оживает, нужен
//! человек». Решение 213: подтверждение тихой пары — info, только при смене состояния пары; ошибка подтверждения —
//! warn с причиной, только при смене причины; тихие пары без подтверждения — info, по паре один раз.
//! Кто вызывает: core/feed_watch.rs.
//! От чего зависит: feed_watch_action.rs, feed_watch_logic.rs (Cfg), engine.rs, gateway (contract), storage/journal.
//! Настройки: значения приходят в Cfg.

use crate::core::types::VenueId;
use crate::gateway::contract::Venue;
use crate::storage::journal::Level;

use super::engine::Core;
use super::feed_watch_logic::{Action, Cfg};

/// Журнал сторожа. Ошибок нет.
pub async fn log(core: &Core, venue: &VenueId, level: Level, msg: String) {
    core.inner
        .deps
        .log(level, "core.feed_watch", Some(&venue.0), msg, None)
        .await;
}

/// Выполняет действие: команда адаптеру, журнал, Telegram. Ошибок нет: сбой команды — в журнал.
/// Action::Check сюда не приходит (его выполняет feed_watch.rs — запрос book_version).
pub async fn act(core: &Core, venue: &VenueId, v: &dyn Venue, a: Action, c: &Cfg) {
    let mult = c.book_stale_ms * c.dead_mult;
    match a {
        Action::Resub {
            conn,
            ticker,
            attempt,
            round,
        } => {
            let r = v.resubscribe_pair(&conn, &ticker).await;
            if attempt == 1 || r.is_err() {
                let tail = r
                    .err()
                    .map(|e| format!(" — не отправлено: {}", e.message))
                    .unwrap_or_default();
                let m = format!(
                    "{conn}: пара {ticker} молчит дольше {mult} мс при живом соединении — переподписка (круг {round}, \
                     попыток в круге {}){tail}",
                    c.attempts
                );
                log(core, venue, Level::Warn, m).await;
            }
        }
        Action::Recovered { conn, ticker } => {
            let m = format!("{conn}: пара {ticker} ожила после переподписки");
            log(core, venue, Level::Info, m).await;
        }
        Action::Reconnect { conn, dead, total } => {
            let r = v.reconnect_conn(&conn).await;
            let tail = r
                .err()
                .map(|e| format!(" — не выполнено: {}", e.message))
                .unwrap_or_default();
            let m = format!(
                "{conn}: мёртвых пар {dead} из {total} (больше {}%), идущих сделок по ним нет — переподключение только этого \
                 соединения{tail}",
                c.dead_pct
            );
            log(core, venue, Level::Warn, m).await;
        }
        Action::Blocked {
            conn,
            dead,
            total,
            deals,
        } => {
            let m = format!(
                "{conn}: мёртвых пар {dead} из {total} (больше {}%), но по парам соединения идут сделки {} — соединение не \
                 переподключаю, переподписка по парам продолжается",
                c.dead_pct,
                deals.join(", ")
            );
            log(core, venue, Level::Warn, m.clone()).await;
            core.inner
                .deps
                .notify("venue.feed_conn_blocked", Some(&venue.0), &m)
                .await;
        }
        Action::GiveUp {
            conn,
            dead,
            total,
            count,
        } => {
            let m = format!(
                "Соединение {conn} биржи {} не оживает: мёртвых пар {dead} из {total}, переподключений за час {count} \
                 (venue.conn_reconnect_max_per_hour) — больше не переподключаю, нужен человек",
                venue.0
            );
            log(core, venue, Level::Error, m.clone()).await;
            core.inner
                .deps
                .notify("venue.feed_conn_giveup", Some(&venue.0), &m)
                .await;
        }
        other => act_quiet(core, venue, v, other, mult).await,
    }
}

/// Решение 213: действия по тихим парам. Ошибок нет.
async fn act_quiet(core: &Core, venue: &VenueId, v: &dyn Venue, a: Action, mult: i64) {
    let (level, m) = match a {
        Action::Confirmed {
            conn,
            ticker,
            until_ms,
            log,
        } => {
            v.book_confirmed(&ticker, until_ms);
            // Решение 227: то же подтверждение — худшей цене ордера ядра (worst.rs: стакан не старше venue.book_stale_ms
            // или подтверждён), иначе ордер тихой пары отказывал бы при котировке, которая прошла.
            super::engine::wr(&core.inner.market).set_quiet(venue, &ticker, until_ms);
            if !log {
                return;
            }
            let m = format!(
                "{conn}: пара {ticker} молчит дольше {mult} мс, но книга на бирже не менялась с последнего кадра \
                 (REST) — тихая, подтверждена: не переподписываю, стакан свежий до следующей проверки"
            );
            (Level::Info, m)
        }
        Action::Unchanged { conn, ticker } => (
            Level::Info,
            format!(
                "{conn}: пара {ticker} — книга на бирже менялась до переподписки и с тех пор не менялась: кадр придёт \
                 при изменении книги, переподписка не поможет; стакана этой версии нет — пара не торгуется до кадра"
            ),
        ),
        Action::Lost { conn, ticker, why } => (
            Level::Info,
            format!("{conn}: пара {ticker} — кадры потеряны ({why}) — переподписка пары"),
        ),
        Action::CheckFailed { conn, ticker, why } => (
            Level::Warn,
            format!("{conn}: пара {ticker} молчит, подтвердить по REST не удалось: {why}"),
        ),
        Action::QuietNoConfirm { conn, why, tickers } => (
            Level::Info,
            format!(
                "{conn}: пары молчат дольше {mult} мс при живом соединении ({}: {}) — {why}; вслепую не \
                 переподписываю",
                tickers.len(),
                tickers.join(", ")
            ),
        ),
        _ => return,
    };
    log(core, venue, level, m).await;
}
