//! Файл: venues/bingx/trades.rs
//! Что делает: соединение ленты сделок одной монеты на время окна (решение 125): подключение к venue.ws_url,
//! подписка «<SYMBOL>@trade», ответ «Pong» на ping, каждый кадр сделки (текст после GZIP) — в приёмник окна
//! (ctx.taps) со временем получения; обрыв — журнал, пауза venue.ws_reconnect_pause_ms, переподключение; окно
//! закрыто (выключатель) — отписка «unsub» и закрытие соединения. Отдельное соединение на монету: в нём одна подписка
//! (предел BingX — 200 на соединение), торговые соединения стаканов не трогаются.
//! Кто вызывает: tap.rs (запуск при открытии окна).
//! От чего зависит: ws.rs (Cfg, config), depth.rs (gunzip, parse), ctx.rs, gateway/tap.rs.
//! Настройки: venue.ws_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms, http.timeout_ms (через ws::config).
//!
//! Официально (docs-v3, Spot → Websocket Market Data → «Subscription transaction by transaction»,
//! https://bingx-api.github.io/docs-v3/#/en/Spot/Websocket%20Market%20Data/Subscription%20transaction%20by%20transaction):
//! dataType «<symbol>@trade», подписка {"id","reqType":"sub","dataType"}, отписка — тот же запрос с reqType «unsub»;
//! «Due to multi-threaded push, it cannot be guaranteed that the push transaction ID is ordered»; каждое сообщение —
//! одна сделка (zh: «逐筆交易推送每一筆成交的信息»). Подписки не чаще 10/с, лимита каналов на IP нет (FAQ-13).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::ws::{Cfg, config};

/// Канал сделок пары по документации: «<symbol>@trade».
pub fn channel(symbol: &str) -> String {
    format!("{symbol}@trade")
}

/// Почему сессия закончилась.
enum End {
    /// Окно закрыто — отписались, работа окончена.
    Stop,
    /// Обрыв — переподключиться.
    Broken(String),
}

/// Работает, пока окно открыто. Принимает: окружение, тикер, символ BingX, выключатель окна. Ошибок нет — в журнал.
pub async fn run(ctx: Ctx, ticker: String, symbol: String, mut stop: watch::Receiver<bool>) {
    let name = format!("trades {symbol}");
    while !*stop.borrow() {
        let end = match config(&ctx) {
            Ok(cfg) => session(&ctx, &name, &ticker, &symbol, &mut stop, cfg).await,
            Err(e) => {
                log(
                    &ctx,
                    Level::Error,
                    format!("лента сделок {symbol} для окна не пишется: {e}"),
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
    symbol: &str,
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
    let ch = channel(symbol);
    let id = format!("{name}-sub");
    let sub = json!({ "id": id, "reqType": "sub", "dataType": ch });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let unsub = json!({ "id": id, "reqType": "unsub", "dataType": ch });
                let _ = sink.send(Message::Text(unsub.to_string().into())).await;
                let _ = sink.close().await;
                return End::Stop;
            }
            next = tokio::time::timeout(cfg.silence, stream.next()) => {
                let frame = match next {
                    Err(_) => return End::Broken(format!("нет сообщений дольше {} мс", cfg.silence.as_millis())),
                    Ok(None) => return End::Broken("соединение закрыто сервером".into()),
                    Ok(Some(Err(e))) => return End::Broken(format!("ошибка чтения: {e}")),
                    Ok(Some(Ok(f))) => f,
                };
                let rx_ns = now_ns();
                let text = match frame {
                    Message::Binary(b) => match depth::gunzip(&b) {
                        Ok(t) => t,
                        Err(e) => return End::Broken(e),
                    },
                    Message::Text(t) => t.to_string(),
                    Message::Close(c) => return End::Broken(format!("сервер закрыл соединение: {c:?}")),
                    _ => continue,
                };
                match depth::parse(&text) {
                    Msg::Ping => {
                        if let Err(e) = sink.send(Message::Text("Pong".into())).await {
                            return End::Broken(format!("отправка Pong: {e}"));
                        }
                    }
                    Msg::SubAck { code, msg, .. } if code != 0 => {
                        log(ctx, Level::Error, format!("WebSocket {name}: подписка {ch} отклонена, код {code}: {msg}")).await;
                    }
                    _ if is_trade(&text, &ch) => {
                        ctx.taps.push(ID, ticker, TapStream::Trades, name, rx_ns, || TapData::Text(text));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Кадр канала сделок: dataType совпадает с подпиской и есть data. Ошибок нет.
fn is_trade(text: &str, ch: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| {
        v.get("dataType").and_then(|d| d.as_str()) == Some(ch)
            && v.get("data").is_some_and(|d| !d.is_null())
    })
}

/// Запись в журнал. Ошибок нет.
async fn log(ctx: &Ctx, level: Level, msg: String) {
    let _ = ctx
        .journal
        .write(level, "venue.ws", Some(ID), msg, None)
        .await;
}
