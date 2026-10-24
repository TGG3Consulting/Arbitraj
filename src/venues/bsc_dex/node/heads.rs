//! Файл: venues/bsc_dex/node/heads.rs
//! Что делает: последний блок сети BSC и его свежесть («не торгуем вслепую»). Решение 57: номер блока опрашивается
//! eth_blockNumber по HTTP раз в bsc.blocks_poll_ms, а не подпиской newHeads по WebSocket: у NodeReal WS считается
//! по байтам (0,04 CU/байт), поток заголовков newHeads ≈ 5,2 КБ/с ≈ до 200 CU/с — больше половины бесплатных
//! 150 CU/с; eth_blockNumber — 5 CU за вызов. Опрос идёт мимо общей очереди темпа узла (rpc::call_unpaced): проверки
//! не отодвигают его, от него зависит свежесть.
//! Свежесть: голова «получена» в момент, когда номер блока вырос; номер стоит дольше feed.stale_after_ms (узел
//! встал или опрос не проходит) — данные устарели, сделок нет. Слушатели получают только новые номера.
//! Время блока (deadline обменов, проба налогов) — отдельным eth_getBlockByNumber (15 CU) только по запросу и не
//! чаще раза на блок (кэш по номеру). Хеши заголовков больше не приходят — реорги видны по removed: true в логах
//! пулов (pools/listen_log.rs) и по квитанциям транзакций (send/track.rs).
//! Кто вызывает: pools/market.rs (запуск при подписке на рынок), quote/*, exec/*, check/tax_pass.rs, send/*,
//! pools/http_wait.rs, pools/session.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, errors.rs.
//! Настройки: bsc.blocks_poll_ms (вся система), feed.stale_after_ms [bsc_dex]; узел — bsc.node_http_url.
//!
//! Источники: eth_blockNumber, eth_getBlockByNumber (поле timestamp — hex-число) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (block.yaml,
//! schemas/block.yaml); цены методов и WS по байтам — https://docs.nodereal.io/docs/compute-units-cus .

use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::U64;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::call::block_tag;
use super::rpc;

/// Голова сети: номер блока и когда мы увидели, что он вырос.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub number: u64,
    /// Когда номер вырос до этого (на нашем сервере).
    pub received_at: DateTime<Utc>,
}

/// Поле timestamp блока из eth_getBlockByNumber.
#[derive(Deserialize)]
struct RawBlock {
    timestamp: U64,
}

/// Последняя голова; дешёво клонируется, слушателей сколько угодно.
#[derive(Clone)]
pub struct Heads {
    tx: watch::Sender<Option<Head>>,
    /// Время последнего запрошенного блока: (номер, секунды Unix).
    time: Arc<Mutex<Option<(u64, u64)>>>,
}

impl Default for Heads {
    fn default() -> Self {
        Self {
            tx: watch::channel(None).0,
            time: Arc::default(),
        }
    }
}

impl Heads {
    /// Приёмник новых номеров блока (трекер транзакций, пулы). Ошибок нет.
    pub fn watch(&self) -> watch::Receiver<Option<Head>> {
        self.tx.subscribe()
    }

    /// Последняя голова без проверки свежести. None — ещё не было. Ошибок нет.
    pub fn latest(&self) -> Option<Head> {
        self.tx.borrow().clone()
    }

    /// Свежая голова: номер вырос не раньше feed.stale_after_ms назад.
    /// Ошибки: StaleData (нет головы или устарела), NotConfigured (порог не задан).
    pub fn fresh(&self, ctx: &Ctx) -> GatewayResult<Head> {
        let limit = ctx.int("feed.stale_after_ms")? as i64;
        let head = self.latest().ok_or_else(|| {
            ctx.err(
                ErrorCode::StaleData,
                "Нет данных о блоках сети BSC (опрос eth_blockNumber)",
            )
        })?;
        let age = (Utc::now() - head.received_at).num_milliseconds();
        if age > limit {
            return Err(ctx.err(
                ErrorCode::StaleData,
                format!("Последний блок {} получен {age} мс назад — больше feed.stale_after_ms ({limit})", head.number),
            ));
        }
        Ok(head)
    }

    /// Время свежего блока (секунды Unix) — для deadline обменов и пробы налогов. Один eth_getBlockByNumber на блок
    /// (кэш по номеру). Ошибки: как у fresh; узел; BadResponse (узел не знает блок или нет timestamp).
    pub async fn timestamp(&self, ctx: &Ctx) -> GatewayResult<u64> {
        let head = self.fresh(ctx)?;
        if let Some((n, t)) = *self.time.lock().unwrap_or_else(|p| p.into_inner())
            && n == head.number
        {
            return Ok(t);
        }
        let b: Option<RawBlock> = rpc::call(
            ctx,
            "eth_getBlockByNumber",
            json!([block_tag(head.number), false]),
        )
        .await?;
        let t = b
            .ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("узел BSC (HTTP) не знает блок {}", head.number),
                )
            })?
            .timestamp
            .to::<u64>();
        *self.time.lock().unwrap_or_else(|p| p.into_inner()) = Some((head.number, t));
        Ok(t)
    }

    /// Принимает номер блока из опроса: вырос — новая голова слушателям; не вырос — ничего (свежесть не
    /// продлевается). Возвращает: была ли новая голова. Ошибок нет.
    pub fn observe(&self, number: u64) -> bool {
        if self.latest().is_some_and(|h| h.number >= number) {
            return false;
        }
        self.tx.send_replace(Some(Head {
            number,
            received_at: Utc::now(),
        }));
        true
    }

    /// Работает постоянно: eth_blockNumber раз в bsc.blocks_poll_ms. Сбой опроса — одна запись на причину и одна
    /// на восстановление; голова при этом стареет (fresh → StaleData). Настройка не задана — запись и выход
    /// (подписка на рынок без неё не начинается — pools/market.rs). Принимает: окружение. Ошибок не возвращает.
    pub async fn run(self, ctx: Ctx) {
        let mut last_err: Option<String> = None;
        loop {
            let every = match poll_every(&ctx) {
                Ok(d) => d,
                Err(e) => {
                    let msg = format!("Опрос блоков остановлен: {}", e.message);
                    ctx.log(Level::Error, "venue.heads", msg, None).await;
                    return;
                }
            };
            match rpc::call_unpaced::<U64>(&ctx, "eth_blockNumber", json!([])).await {
                Ok(n) => {
                    self.observe(n.to());
                    if last_err.take().is_some() {
                        let msg = "Опрос блоков снова работает".to_string();
                        ctx.log(Level::Info, "venue.heads", msg, None).await;
                    }
                }
                Err(e) if last_err.as_deref() != Some(e.message.as_str()) => {
                    let msg = format!("Опрос блоков: {}", e.message);
                    ctx.log(Level::Warn, "venue.heads", msg, None).await;
                    last_err = Some(e.message);
                }
                Err(_) => {}
            }
            tokio::time::sleep(every).await;
        }
    }
}

/// Интервал опроса блоков: bsc.blocks_poll_ms (вся система), строго меньше feed.stale_after_ms — иначе голова
/// «устаревала» бы между опросами. Ошибки: NotConfigured (не задана или не меньше порога свежести).
pub fn poll_every(ctx: &Ctx) -> GatewayResult<Duration> {
    let ms = ctx
        .settings
        .integer("bsc.blocks_poll_ms", crate::settings::read::At::default())
        .map_err(|e| ctx.tag(e))?;
    let stale = ctx.int("feed.stale_after_ms")?;
    if ms >= stale {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            format!("bsc.blocks_poll_ms ({ms}) должен быть меньше feed.stale_after_ms ({stale})"),
        ));
    }
    Ok(Duration::from_millis(ms))
}
