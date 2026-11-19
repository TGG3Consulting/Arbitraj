//! Файл: venues/binance/sbe_run.rs
//! Что делает: решение 425 — быстрый стакан Binance по SBE: настройки (адрес, ключ Ed25519, очередь снимков), подключение
//! с заголовком X-MBX-APIKEY, запуск соединения SBE для пачки монет рядом с соединением JSON (market.rs) и его жизненный
//! цикл: обрыв — все пары соединения на страховку JSON (fast.rs), журнал, пауза venue.ws_reconnect_pause_ms, заново.
//! Нет адреса, ключа или настроек очереди — соединение SBE не запускается, стакан — JSON, как раньше (запись журнала
//! venue.book_fast с причиной; «Проверить подключение» тоже пишет причину — sbe_check.rs).
//! Кто вызывает: market.rs (start), sbe_check.rs (connect, url).
//! От чего зависит: sbe_conn.rs, sbe_snap.rs (config), ws.rs (Cfg: темп подписок, потоков в SUBSCRIBE, тишина, таймаут
//! подключения, уровни), fast.rs, book.rs, ctx.rs (секрет binance.sbe_api_key), tokio-tungstenite.
//! Настройки: venue.sbe_ws_url, venue.sbe_snapshot_per_sec, venue.sbe_snapshot_limit [binance]; из ws.rs —
//! venue.ws_subs_per_sec, venue.ws_streams_per_msg, venue.ws_silence_ms, venue.depth_levels, http.timeout_ms;
//! venue.ws_reconnect_pause_ms.
//!
//! Официально (sbe-market-data-streams.md «General Information», «WebSocket Limits»): адрес stream-sbe.binance.com или
//! stream-sbe.binance.com:9443; «/stream?streams=…» или живая подписка («Live Subscribing and Unsubscribing is also
//! supported», запросы — JSON в текстовых кадрах); «An API Key is necessary for access. Only Ed25519 keys are allowed.
//! Please put your API Key in the X-MBX-APIKEY header when opening the connection. Timestamp and signature are not
//! necessary»; соединение живёт 24 ч; 5 сообщений клиента в секунду (PING, PONG, JSON-запрос), до 1024 потоков на
//! соединение, 300 попыток подключения за 5 минут с IP — те же пределы, что у JSON (ws.rs, market.rs): соединений SBE
//! столько же, сколько JSON, темп подписок — тот же venue.ws_subs_per_sec.

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use zeroize::Zeroizing;

use crate::core::types::Asset;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::{Ctx, SECRET_SBE_API_KEY};
use super::fast::Fast;
use super::sbe_conn;
use super::ws::{self, End};

/// Соединение WebSocket.
pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Настройки соединения SBE.
pub struct SbeCfg {
    /// venue.sbe_ws_url + /stream.
    pub url: String,
    pub key: Zeroizing<String>,
    /// Темп подписок, потоков в SUBSCRIBE, тишина, таймаут подключения, уровни ядру — как у JSON.
    pub base: ws::Cfg,
    pub resync_pause: Duration,
}

/// Что делает: адрес комбинированного потока SBE. Ошибка — текст (venue.sbe_ws_url не задана).
pub fn url(ctx: &Ctx) -> Result<String, String> {
    let u = ctx.text("venue.sbe_ws_url").map_err(|e| e.message)?;
    Ok(format!("{}/stream", u.trim_end_matches('/')))
}

/// Что делает: все настройки SBE. Ошибка — текст причины (не задано, ключ не введён или секреты закрыты, вне пределов).
pub fn config(ctx: &Ctx) -> Result<SbeCfg, String> {
    let url = url(ctx)?;
    let key = ctx
        .vault
        .get(SECRET_SBE_API_KEY)
        .map_err(|e| format!("ключ Ed25519 для SBE ({SECRET_SBE_API_KEY}): {}", e.message))?;
    super::sbe_snap::config(ctx)?;
    let base = ws::config(ctx)?;
    let pause = ctx
        .int("venue.ws_reconnect_pause_ms")
        .map_err(|e| e.message)?;
    Ok(SbeCfg {
        url,
        key,
        base,
        resync_pause: Duration::from_millis(pause),
    })
}

/// Что делает: подключение с ключом в заголовке X-MBX-APIKEY (подписи нет — так в документации). Принимает: адрес,
/// ключ, таймаут. Возвращает: соединение. Ошибки: текст — точная причина (ответ HTTP при отказе: код и тело).
pub async fn connect(url: &str, key: &str, timeout: Duration) -> Result<Ws, String> {
    let mut req = url
        .into_client_request()
        .map_err(|e| format!("адрес {url}: {e}"))?;
    let v = HeaderValue::from_str(key)
        .map_err(|_| "ключ содержит недопустимые для заголовка символы".to_string())?;
    req.headers_mut().insert("X-MBX-APIKEY", v);
    match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(req)).await {
        Err(_) => Err("не удалось подключиться за отведённое время".into()),
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(resp))) => {
            let body = resp
                .body()
                .as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            Err(format!("биржа отказала: HTTP {} {body}", resp.status()))
        }
        Ok(Err(e)) => Err(format!("подключение: {e}")),
    }
}

/// Что делает: арбитр пачки и, если SBE настроен, задача соединения SBE. Принимает: окружение, имя соединения JSON,
/// монеты пачки, стаканы, канал ядра, первая ли пачка подписки (запись «не включён» — одна на подписку).
/// Возвращает: арбитр (выключенный — без SBE) и задачу. Ошибок нет: SBE не настроен — запись журнала, стакан JSON как
/// раньше.
pub async fn start(
    ctx: &Ctx,
    name: &str,
    assets: &[(Asset, String)],
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    first: bool,
) -> (Fast, Option<JoinHandle<()>>) {
    if let Err(why) = config(ctx) {
        if first {
            let m = format!(
                "Binance: быстрый стакан SBE не включён — {why}; стакан — JSON, как раньше"
            );
            ctx.log(Level::Info, "venue.book_fast", m).await;
        }
        return (Fast::default(), None);
    }
    let levels = ws::config(ctx)
        .map(|c| c.levels as usize)
        .unwrap_or_default();
    let tickers: Vec<String> = assets.iter().map(|(a, _)| a.ticker.clone()).collect();
    let fast = Fast::new(ctx, name, &tickers, books, out, levels);
    let task = tokio::spawn(run(
        ctx.clone(),
        format!("{name}-sbe"),
        assets.to_vec(),
        fast.clone(),
        out.clone(),
    ));
    (fast, Some(task))
}

/// Работает, пока жив канал ядра: подключается заново после каждого обрыва. Ошибок нет.
async fn run(
    ctx: Ctx,
    name: String,
    assets: Vec<(Asset, String)>,
    fast: Fast,
    out: mpsc::Sender<MarketEvent>,
) {
    while !out.is_closed() {
        let end = match config(&ctx) {
            Ok(cfg) => sbe_conn::session(&ctx, &name, &assets, &fast, cfg).await,
            Err(why) => End::Config(why),
        };
        fast.set_resync(None);
        let why = match &end {
            End::Done => return,
            End::Broken(r) => format!("обрыв SBE: {r}"),
            End::Config(r) => format!("SBE остановлен: {r}"),
        };
        for (a, _) in &assets {
            fast.fast_lost(a, &why, false);
        }
        let pause = ctx.int("venue.ws_reconnect_pause_ms");
        let (level, text, pause) = match (end, pause) {
            (End::Broken(_), Ok(p)) => (
                Level::Warn,
                format!("WebSocket {name}: {why}; переподключение"),
                Some(p),
            ),
            (_, _) => (
                Level::Error,
                format!("WebSocket {name}: {why}; стакан — JSON"),
                None,
            ),
        };
        ctx.log(level, "venue.ws", text).await;
        match pause {
            Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            None => return,
        }
    }
}
