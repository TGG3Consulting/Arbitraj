//! Файл: venues/bybit/ws_lane.rs
//! Что делает: кадр стакана Bybit в сессии (ws_book.rs) — локальный стакан пары (local_book.rs) и решение 428:
//! основной поток отдаёт стакан ядру через выбор источника (backup.rs), страховка — запоминает свой стакан и отдаёт его,
//! пока основной не синхронизирован. Разрыв u — пересподписка пары за новым снимком (второй разрыв раньше
//! venue.ws_reconnect_pause_ms — переподключение, урок 12); у основного при этом сразу стакан страховки (ws_give.rs).
//! Расхождение при равном seq — основной переподписывается (снимок), стакан ядра — страховки.
//! Кто вызывает: ws_book.rs.
//! От чего зависит: backup.rs, backup_log.rs, ws_give.rs, local_book.rs, book_print.rs (U_LEVELS), book.rs,
//! gateway/feed_health.rs (Cmd::Resub основному соединению), ctx.rs.
//! Настройки: venue.book_seq_strict [bybit] — u строго +1.
//!
//! Официально (Orderbook https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook): «If you receive a new
//! snapshot message, you will have to reset your local orderbook»; после подписки — snapshot («Once you have subscribed
//! successfully, you will receive a snapshot»); Connect («Understanding WebSocket Filters: Unsubscription»): «You can
//! dynamically subscribe and unsubscribe from topics without unsubscribing from the WebSocket». Решение 202/213: номер u
//! кадра — сторожу ядра только при глубине book_print::U_LEVELS и только когда стакан ядра — из основного потока.

use serde_json::json;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::core_out_book::Fed;
use crate::gateway::feed_health::Cmd;
use crate::storage::journal::Level;

use super::backup::{BackupAct, Held, Main, MainAct, print};
use super::backup_log::Ev;
use super::book_print::U_LEVELS;
use super::depth::channel;
use super::local_book::{Applied, LocalBook};
use super::ws::End;
use super::ws_book::{St, Step};
use super::ws_give::{self, give};

/// Кадр стакана (поля разобранного кадра depth.rs).
pub struct Frame<'t> {
    pub symbol: &'t str,
    pub snapshot: bool,
    pub u: i64,
    pub seq: Option<i64>,
    pub ts: Option<i64>,
    pub bids: &'t [[&'t str; 2]],
    pub asks: &'t [[&'t str; 2]],
}

/// Шаг по итогу отдачи ядру. Ошибок нет.
fn fed_step(st: &mut St<'_>, fed: Option<Fed>) -> Step {
    match fed {
        Some(Fed::Closed) => Step {
            send: vec![],
            end: Some(End::Done),
        },
        Some(Fed::Lost) => {
            st.dropped += 1;
            Step::default()
        }
        _ => Step::default(),
    }
}

/// Кадр стакана пары. Принимает: состояние сессии, монету, кадр. Возвращает: Step. Ошибок нет.
pub async fn on_book(st: &mut St<'_>, asset: &Asset, f: Frame<'_>) -> Step {
    // Без новой строки ключа на каждый кадр (урок 13): вставка — только у первого кадра пары.
    if !st.local.contains_key(f.symbol) {
        st.local.insert(f.symbol.to_string(), LocalBook::default());
    }
    let strict = matches!(st.ctx.flag("venue.book_seq_strict"), Ok(true));
    let Some(lb) = st.local.get_mut(f.symbol) else {
        return Step::default();
    };
    let applied = lb.apply(f.snapshot, f.u, strict, f.bids, f.asks);
    let ins = st.ins;
    let (hub, ticker) = (&ins.hub, asset.ticker.as_str());
    let top = matches!(applied, Applied::Changed).then(|| lb.top(st.cfg.depth));
    let u_ok = st.cfg.levels == U_LEVELS;
    match (applied, top) {
        (Applied::Changed, Some((bids, asks))) => {
            let p = if hub.on() {
                print(&bids, &asks, hub.cmp())
            } else {
                (0, 0)
            };
            let book = OrderBook {
                venue: st.venue.clone(),
                asset: asset.clone(),
                bids,
                asks,
                exchange_time: f.ts.and_then(chrono::DateTime::from_timestamp_millis),
                received_at: chrono::Utc::now(),
            };
            let mut fed = None;
            let (books, kept, out, alive) = (st.books, &st.kept, st.out, st.alive.clone());
            if st.backup {
                let Some(seq) = f.seq else {
                    return Step::default();
                };
                let held = Held {
                    book,
                    alive,
                    seq,
                    at: std::time::Instant::now(),
                };
                let act = hub.backup_frame(ticker, held, p, |h| {
                    fed = Some(give(books, kept, out, h.book.clone(), h.alive.clone()))
                });
                if let BackupAct::Mismatch(conn) = act {
                    mismatched(st, ticker, conn);
                }
                return fed_step(st, fed);
            }
            let act = hub.main_frame(ticker, f.snapshot, f.seq, p, || {
                fed = Some(give(books, kept, out, book, alive))
            });
            let from_main = matches!(act, MainAct::Give | MainAct::Back);
            if let Some(feed) = st.feed {
                feed.frame_ver(ticker, (u_ok && from_main).then_some(f.u), None);
            }
            match act {
                MainAct::Back => {
                    let why = "основной синхронизирован (снимок)";
                    st.ins
                        .stats
                        .hit(st.ctx, Ev::ToMain, ticker, why, hub.compares());
                }
                MainAct::Mismatch => {
                    st.ins.stats.hit(
                        st.ctx,
                        Ev::Mismatch,
                        ticker,
                        "первые уровни при равном seq",
                        hub.compares(),
                    );
                    return resync(
                        st,
                        f.symbol,
                        ticker,
                        "расхождение со страховкой при равном seq",
                        false,
                    )
                    .await;
                }
                _ => {}
            }
            fed_step(st, fed)
        }
        (Applied::Repeat, _) => {
            if st.backup {
                if hub.backup_touch(ticker) {
                    st.books.touch(ticker);
                }
            } else {
                let src = hub.main_is_source(ticker);
                if let Some(feed) = st.feed {
                    feed.frame_ver(ticker, (u_ok && src).then_some(f.u), None);
                }
                if src {
                    st.books.touch(ticker);
                }
            }
            Step::default()
        }
        (Applied::Gap(why), _) => {
            // Без страховки — как раньше (u кадра); со страховкой стакан ядра уже не из этого кадра.
            if let Some(feed) = st.feed {
                feed.frame_ver(ticker, (u_ok && !hub.on()).then_some(f.u), None);
            }
            resync(st, f.symbol, ticker, &why, true).await
        }
        (Applied::Changed, None) => Step::default(),
    }
}

/// Сверка у страховки показала расхождение: счётчики и команда основному соединению «переподписать пару» (новый
/// снимок; стакан ядра уже из страховки). Ошибок нет: соединения нет — запись журнала.
fn mismatched(st: &St<'_>, ticker: &str, conn: Option<String>) {
    let (c, why) = (
        st.ins.hub.compares(),
        "расхождение с основным при равном seq",
    );
    st.ins.stats.hit(st.ctx, Ev::Mismatch, ticker, why, c);
    st.ins.stats.hit(st.ctx, Ev::ToBackup, ticker, why, c);
    st.ins.stats.hit(st.ctx, Ev::Resync, ticker, why, c);
    match conn.map(|n| st.books.feed.send(&n, Cmd::Resub(ticker.to_string()))) {
        Some(Ok(())) => {}
        Some(Err(e)) => tracing::warn!(
            ticker,
            "страховка Bybit: переподписка основного не отправлена: {e}"
        ),
        None => tracing::warn!(
            ticker,
            "страховка Bybit: соединение основного потока пары неизвестно"
        ),
    }
}

/// Пересподписка пары за новым снимком. Принимает: состояние, символ, тикер, причину, разрыв ли номера (да — второй
/// разрыв раньше venue.ws_reconnect_pause_ms — переподключение; нет — расхождение: локальный стакан не сбрасывается,
/// изменения до снимка ядру не идут). Возвращает: Step с unsubscribe + subscribe или концом сессии. Ошибок нет.
async fn resync(st: &mut St<'_>, symbol: &str, ticker: &str, why: &str, gap: bool) -> Step {
    let topic = channel(st.cfg.levels, symbol);
    if st.backup {
        ws_give::backup_lost(st.ctx, st.books, st.out, st.ins, ticker, why);
    } else {
        ws_give::main_lost(st.ctx, st.books, st.out, st.ins, ticker, Main::Resync, why);
    }
    if gap {
        if st
            .resynced
            .get(symbol)
            .is_some_and(|at| at.elapsed() < st.cfg.resync_gap)
        {
            let end = End::Broken(format!(
                "{topic}: снова разрыв стакана ({why}) — переподключение"
            ));
            return Step {
                send: vec![],
                end: Some(end),
            };
        }
        st.resynced
            .insert(symbol.to_string(), tokio::time::Instant::now());
    }
    st.log(
        Level::Warn,
        format!("{topic}: {why} — пересподписка за новым снимком"),
    )
    .await;
    let id = format!("r-{symbol}");
    st.sent.insert(id.clone(), vec![topic.clone()]);
    let send = ["unsubscribe", "subscribe"]
        .iter()
        .map(|op| json!({ "op": op, "req_id": id, "args": [topic.clone()] }).to_string())
        .collect();
    Step { send, end: None }
}
