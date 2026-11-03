//! Файл: venues/mexc/trades.rs
//! Что делает: соединение ленты сделок одной монеты на время окна (решение 125): подключение к venue.ws_url,
//! {"method":"SUBSCRIPTION"} канала сделок, свой {"method":"PING"} раз в venue.ws_ping_ms, Pong на Ping-кадр, каждый
//! двоичный кадр (protobuf как есть) — в приёмник окна (ctx.taps) со временем получения; обрыв — журнал, пауза
//! venue.ws_reconnect_pause_ms, переподключение; окно закрыто — {"method":"UNSUBSCRIPTION"} и закрытие. Отдельное
//! соединение на монету: в нём одна подписка (предел MEXC — 30 на соединение), торговые соединения не трогаются.
//! Кто вызывает: tap.rs (запуск при открытии окна).
//! От чего зависит: ws.rs (Cfg, config), depth.rs (parse_text), ctx.rs, gateway/tap.rs.
//! Настройки: venue.trades_interval [mexc] (10ms / 100ms — часть имени канала), venue.ws_url, venue.ws_silence_ms,
//! venue.ws_ping_ms, venue.ws_reconnect_pause_ms, http.timeout_ms (через ws::config).
//!
//! Официально (https://www.mexc.com/api-docs/spot-v3/websocket-market-streams/trade-streams и исходник документации
//! mexcdevelop/apidocs, ветка spot_v3_en, раздел «Trade Streams»): канал «spot@public.aggre.deals.v3.api.pb@(100ms|10ms)
//! @<symbol>», символ заглавными; «The Trade Streams push raw trade information for each fill»; каждая сделка — элемент
//! списка deals (цена, количество, tradeType, время, tradeId), кадр несёт сделки за интервал. Отписка —
//! {"method":"UNSUBSCRIPTION","params":[канал]} (раздел «Live Subscription/Unsubscription to Data Streams»). Не больше 30
//! подписок на соединение; без подписки соединение закрывается через 30 с, без данных — через минуту.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, config};

/// Канал сделок по документации. Принимает: интервал (venue.trades_interval), символ MEXC.
pub fn channel(interval: &str, symbol: &str) -> String {
    format!("spot@public.aggre.deals.v3.api.pb@{interval}@{symbol}")
}

/// Почему сессия закончилась.
enum End {
    Stop,
    Broken(String),
}

/// Работает, пока окно открыто. Принимает: окружение, тикер, канал, выключатель окна. Ошибок нет — в журнал.
pub async fn run(ctx: Ctx, ticker: String, ch: String, mut stop: watch::Receiver<bool>) {
    let name = format!("trades {ticker}");
    while !*stop.borrow() {
        let end = match config(&ctx) {
            Ok(cfg) => session(&ctx, &name, &ticker, &ch, &mut stop, cfg).await,
            Err(e) => {
                log(
                    &ctx,
                    Level::Error,
                    format!("лента сделок {ticker} для окна не пишется: {e}"),
                )
                .await;
                return;
            }
        };
        let r = match end {
            End::Stop => return,
            End::Broken(r) => r,
        };
        let Ok(pause) = ctx.int("venue.ws_reconnect_pause_ms") else {
            log(&ctx, Level::Error, format!("WebSocket {name}: {r}; не задана venue.ws_reconnect_pause_ms — лента окна остановлена")).await;
            return;
        };
        log(
            &ctx,
            Level::Warn,
            format!("WebSocket {name}: {r}; переподключение (в записи окна — пропуск)"),
        )
        .await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(pause)) => {}
            _ = stop.changed() => return,
        }
    }
}

/// Одна сессия ленты сделок. Возвращает причину окончания.
async fn session(
    ctx: &Ctx,
    name: &str,
    ticker: &str,
    ch: &str,
    stop: &mut watch::Receiver<bool>,
    cfg: Cfg,
) -> End {
    let connect = tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(cfg.url.as_str()),
    );
    let ws = tokio::select! {
        r = connect => match r {
            Err(_) => return End::Broken("не удалось подключиться за отведённое время".into()),
            Ok(Err(e)) => return End::Broken(format!("подключение: {e}")),
            Ok(Ok((ws, _))) => ws,
        },
        _ = stop.changed() => return End::Stop,
    };
    let (mut sink, mut stream) = ws.split();
    let sub = json!({ "method": "SUBSCRIPTION", "params": [ch] });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    let mut ping = tokio::time::interval(cfg.ping);
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let unsub = json!({ "method": "UNSUBSCRIPTION", "params": [ch] });
                let _ = sink.send(Message::Text(unsub.to_string().into())).await;
                let _ = sink.close().await;
                return End::Stop;
            }
            _ = ping.tick() => {
                let p = json!({ "method": "PING" });
                if let Err(e) = sink.send(Message::Text(p.to_string().into())).await {
                    return End::Broken(format!("отправка PING: {e}"));
                }
            }
            _ = tokio::time::sleep_until(last_rx + cfg.silence) => {
                return End::Broken(format!("нет кадров дольше {} мс", cfg.silence.as_millis()));
            }
            next = stream.next() => {
                let frame = match next {
                    None => return End::Broken("соединение закрыто сервером".into()),
                    Some(Err(e)) => return End::Broken(format!("ошибка чтения: {e}")),
                    Some(Ok(f)) => f,
                };
                let rx_ns = now_ns();
                last_rx = Instant::now();
                match frame {
                    // В соединении одна подписка — каждый двоичный кадр относится к ленте сделок этой монеты.
                    Message::Binary(b) => {
                        ctx.taps.push(ID, ticker, TapStream::Trades, name, rx_ns, || TapData::Binary(b.to_vec()));
                    }
                    Message::Text(t) => {
                        if let Msg::Ack { code, msg } = depth::parse_text(&t)
                            && code != 0
                        {
                            log(ctx, Level::Error, format!("WebSocket {name}: отказ подписки {ch}, код {code}: {msg}")).await;
                        }
                    }
                    Message::Ping(p) => {
                        if let Err(e) = sink.send(Message::Pong(p)).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                    }
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => {}
                }
            }
        }
    }
}

/// Запись в журнал. Ошибок нет.
async fn log(ctx: &Ctx, level: Level, msg: String) {
    let _ = ctx
        .journal
        .write(level, "venue.ws", Some(ID), msg, None)
        .await;
}
