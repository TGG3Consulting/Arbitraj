//! Файл: venues/bingx/feed.rs
//! Что делает: соединение окна (решение 125) для одной монеты и одного канала на время окна — лента сделок
//! «<SYMBOL>@trade» или инкрементальный стакан всей глубины «<SYMBOL>@incrDepth»: подключение к venue.ws_url, подписка,
//! «Pong» на ping, каждый кадр канала (текст после GZIP) — в приёмник окна (ctx.taps) со временем получения; обрыв —
//! журнал, пауза venue.ws_reconnect_pause_ms, переподключение; окно закрыто — отписка «unsub» и закрытие. У @incrDepth
//! разрыв lastUpdateId (N-й ≠ (N-1)-й + 1) — переподключение: после подписки BingX снова шлёт полный стакан
//! (action «all»), он тоже пишется. Своё соединение на канал монеты: одна подписка (предел BingX — 200 на соединение),
//! торговые соединения не трогаются.
//! Кто вызывает: tap.rs (запуск при открытии окна).
//! От чего зависит: ws.rs (Cfg, config), depth.rs (gunzip, parse), tap_diff.rs (of_channel, gap), ctx.rs, gateway/tap.rs.
//! Настройки: venue.ws_url, venue.ws_silence_ms, venue.ws_reconnect_pause_ms, http.timeout_ms (через ws::config).
//!
//! Официально (docs-v3, Spot → Websocket Market Data; https://bingx-api.github.io/docs-v3/#/en/Spot/Websocket%20Market%20Data/):
//! - «Subscription transaction by transaction»: dataType «<symbol>@trade», одна сделка в сообщении, «it cannot be
//!   guaranteed that the push transaction ID is ordered»;
//! - «Incremental and Full Depth Information»: dataType «<symbol>@incrDepth», «Push incremental depth information of
//!   1000 levels every 500ms»; первым — полный стакан action «all» с lastUpdateId, дальше action «update»,
//!   lastUpdateId N-го = N-1-го + 1; «In rare cases where lastUpdateId is not continuous, you can choose to reconnect».
//!
//! Подписка {"id","reqType":"sub","dataType"}, отписка — reqType «unsub». Подписки не чаще 10/с, лимита каналов на IP
//! нет (FAQ-13).

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

/// Какой канал окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Лента сделок «<symbol>@trade».
    Trades,
    /// Инкрементальный стакан всей глубины «<symbol>@incrDepth».
    Diff,
}

impl Feed {
    /// Канал по документации. Принимает: символ BingX.
    pub fn channel(self, symbol: &str) -> String {
        match self {
            Feed::Trades => format!("{symbol}@trade"),
            Feed::Diff => format!("{symbol}@incrDepth"),
        }
    }

    /// Поток записи окна. Ошибок нет.
    fn stream(self) -> TapStream {
        match self {
            Feed::Trades => TapStream::Trades,
            Feed::Diff => TapStream::Diff,
        }
    }
}

/// Почему сессия закончилась.
enum End {
    /// Окно закрыто — отписались, работа окончена.
    Stop,
    /// Обрыв — переподключиться.
    Broken(String),
}

/// Работает, пока окно открыто. Принимает: окружение, тикер, символ BingX, канал, выключатель окна. Ошибок нет.
pub async fn run(
    ctx: Ctx,
    ticker: String,
    symbol: String,
    feed: Feed,
    mut stop: watch::Receiver<bool>,
) {
    let ch = feed.channel(&symbol);
    let name = format!("window {ch}");
    while !*stop.borrow() {
        let end = match config(&ctx) {
            Ok(cfg) => session(&ctx, &name, &ticker, &ch, feed, &mut stop, cfg).await,
            Err(e) => {
                log(
                    &ctx,
                    Level::Error,
                    format!("канал {ch} для окна не пишется: {e}"),
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
            log(&ctx, Level::Error, format!("WebSocket {name}: {r}; не задана venue.ws_reconnect_pause_ms — канал окна остановлен")).await;
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

/// Одна сессия канала. Возвращает причину окончания.
async fn session(
    ctx: &Ctx,
    name: &str,
    ticker: &str,
    ch: &str,
    feed: Feed,
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
    let id = format!("{name}-sub");
    let sub = json!({ "id": id, "reqType": "sub", "dataType": ch });
    if let Err(e) = sink.send(Message::Text(sub.to_string().into())).await {
        return End::Broken(format!("отправка подписки: {e}"));
    }
    // Последний lastUpdateId канала @incrDepth (разрыв — переподключение).
    let mut last: Option<i64> = None;
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
                    _ => {
                        let Some(v) = super::tap_diff::of_channel(&text, ch) else { continue };
                        let gap = if feed == Feed::Diff { super::tap_diff::gap(&v, &mut last) } else { None };
                        ctx.taps.push(ID, ticker, feed.stream(), name, rx_ns, || TapData::Text(text));
                        if let Some(why) = gap {
                            return End::Broken(why);
                        }
                    }
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
