//! Файл: venues/bsc_dex/node/heads.rs
//! Что делает: последний блок сети BSC и его свежесть («не торгуем вслепую»). Решение 57: номер блока опрашивается
//! eth_blockNumber по HTTP раз в bsc.blocks_poll_ms, а не подпиской newHeads по WebSocket: у NodeReal WS считается
//! по байтам (0,04 CU/байт), поток заголовков newHeads ≈ 5,2 КБ/с ≈ до 200 CU/с — больше половины бесплатных
//! 150 CU/с; eth_blockNumber — 5 CU за вызов. Опрос идёт мимо общей очереди темпа узла (rpc::call_unpaced): проверки
//! не отодвигают его, от него зависит свежесть.
//! Свежесть: голова «получена» в момент, когда номер блока вырос; номер стоит дольше feed.stale_after_ms (узел
//! встал или опрос не проходит) — данные устарели, сделок нет. Слушатели получают только новые номера.
//! Время блока (deadline, проба) — eth_getBlockByNumber (15 CU) по запросу, раз на блок на всех (кэш по номеру,
//! одновременные промахи ждут один запрос). Решение 121 (A2): задан feed.max_block_age_ms — время каждой новой головы
//! берётся сразу в опросе, и fresh() отказывает (StaleData), если блок старше порога (node/head_age.rs). Реорги — по removed: true в логах пулов и по квитанциям (send/track.rs).
//! Кто вызывает: pools/market.rs (запуск при подписке на рынок), quote/*, exec/*, check/tax_pass.rs, send/*,
//! pools/http_wait.rs, pools/session.rs.
//! От чего зависит: ctx.rs, node/rpc.rs, node/head_age.rs, errors.rs.
//! Настройки: bsc.blocks_poll_ms (вся система), feed.stale_after_ms, feed.max_block_age_ms [bsc_dex]; узел —
//! bsc.node_http_url.
//!
//! Источники: eth_blockNumber, eth_getBlockByNumber (поле timestamp — hex-число) — execution-apis
//! https://github.com/ethereum/execution-apis/tree/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth (block.yaml,
//! schemas/block.yaml); цены методов и WS по байтам — https://docs.nodereal.io/docs/compute-units-cus .

use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::U64;
use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::watch;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::rpc;

/// Голова сети: номер блока и когда мы увидели, что он вырос.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    pub number: u64,
    /// Когда номер вырос до этого (на нашем сервере).
    pub received_at: DateTime<Utc>,
}

/// Последняя голова; дешёво клонируется, слушателей сколько угодно.
#[derive(Clone)]
pub struct Heads {
    tx: watch::Sender<Option<Head>>,
    /// Время последнего запрошенного блока: (номер, секунды Unix).
    time: Arc<Mutex<Option<(u64, u64)>>>,
    /// Один eth_getBlockByNumber на блок на всех: одновременные промахи ждут первого (решение 61).
    flight: Arc<tokio::sync::Mutex<()>>,
}

impl Default for Heads {
    fn default() -> Self {
        Self {
            tx: watch::channel(None).0,
            time: Arc::default(),
            flight: Arc::default(),
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

    /// Свежая голова: номер вырос не раньше feed.stale_after_ms назад и (если задан feed.max_block_age_ms) время
    /// последнего известного блока не старше порога (node/head_age.rs).
    /// Ошибки: StaleData (нет головы, номер не растёт или блок старый), NotConfigured (порог не задан).
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
        super::head_age::check(ctx, self.block_time())?;
        Ok(head)
    }

    /// Свежий блок и его время (секунды Unix): номер — для симуляции на этом же блоке (eth_simulateV1), время — для
    /// deadline. Один eth_getBlockByNumber на блок на всех (кэш по номеру; одновременные промахи ждут один запрос).
    /// Ошибки: как у fresh; узел; BadResponse (узел не знает блок или нет timestamp).
    pub async fn at(&self, ctx: &Ctx) -> GatewayResult<(u64, u64)> {
        let head = self.fresh(ctx)?;
        let hit = || {
            self.block_time()
                .filter(|(n, _)| *n == head.number)
                .map(|(_, t)| t)
        };
        if let Some(t) = hit() {
            return Ok((head.number, t));
        }
        // Последнее известное (номер, время) любого блока — чтобы расчёт не ждал узел (решение 89).
        let last = || self.block_time();
        match self.flight.try_lock() {
            // Ведущий: на этом блоке кэша нет и никто не тянет — обновляем с узла.
            Ok(_one) => match hit() {
                Some(t) => Ok((head.number, t)),
                None => self.fetch_time(ctx, head.number).await,
            },
            // Кто-то уже тянет (обычно фоновый прогрев): берём последнее известное, не блокируемся на узле.
            Err(_) => match last() {
                Some((n, t)) => Ok((n, t)),
                // Холодный старт: ничего нет — один раз дожидаемся того запроса.
                None => {
                    let _one = self.flight.lock().await;
                    match hit() {
                        Some(t) => Ok((head.number, t)),
                        None => self.fetch_time(ctx, head.number).await,
                    }
                }
            },
        }
    }

    /// Обновляет кэш времени блока с узла (eth_getBlockByNumber). Держится под flight — один запрос на всех.
    /// Принимает: окружение, номер блока. Возвращает: (номер, время Unix). Ошибки: узел; BadResponse (нет блока/timestamp).
    async fn fetch_time(&self, ctx: &Ctx, number: u64) -> GatewayResult<(u64, u64)> {
        let t = super::head_age::fetch(ctx, number, true).await?;
        self.remember_time(number, t);
        Ok((number, t))
    }

    /// Последнее известное время блока: (номер, секунды Unix). None — ещё не было. Ошибок нет.
    pub fn block_time(&self) -> Option<(u64, u64)> {
        *self.time.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Запоминает время блока, если он не старше уже известного (запросы могут вернуться не по порядку).
    /// Принимает: номер блока, время (секунды Unix). Ошибок нет.
    pub fn remember_time(&self, number: u64, t: u64) {
        let mut cur = self.time.lock().unwrap_or_else(|p| p.into_inner());
        if cur.is_none_or(|(n, _)| number >= n) {
            *cur = Some((number, t));
        }
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
    /// на восстановление; голова при этом стареет (fresh → StaleData). Настройка не задана — запись и ожидание её
    /// задания (settings_wait.rs). Принимает: окружение. Ошибок не возвращает.
    pub async fn run(self, ctx: Ctx) {
        let mut last_err: Option<String> = None;
        let mut age_log = super::head_age::AgeLog::default();
        loop {
            // Не задана (или неверна) — опрос ждёт настройку и продолжает сам (settings_wait.rs), без перезапуска.
            let read = || poll_every(&ctx);
            let Some(every) =
                super::super::settings_wait::until_ok(&ctx, "Опрос блоков", "venue.heads", read)
                    .await
            else {
                return;
            };
            match rpc::call_unpaced::<U64>(&ctx, "eth_blockNumber", json!([])).await {
                Ok(n) => {
                    if self.observe(n.to()) {
                        super::head_age::time_of_new_head(&self, &ctx, n.to()).await;
                    }
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
            // Возраст головы — каждый круг опроса, и при сбое опроса тоже: голова стареет — запись о переходе.
            let age = match super::head_age::max_age_ms(&ctx) {
                Ok(Some(_)) => Some(super::head_age::check(&ctx, self.block_time())),
                _ => None,
            };
            age_log.step(&ctx, age.as_ref()).await;
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
        return Err(ctx.not_set(
            "bsc.blocks_poll_ms",
            format!("bsc.blocks_poll_ms ({ms}) должен быть меньше feed.stale_after_ms ({stale})"),
        ));
    }
    Ok(Duration::from_millis(ms))
}
