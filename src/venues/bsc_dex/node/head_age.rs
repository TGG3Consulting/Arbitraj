//! Файл: venues/bsc_dex/node/head_age.rs
//! Что делает: возраст головы сети по времени блока (решение 121, часть A2): возраст = часы сервера − timestamp
//! последнего известного блока. Номер блока у догоняющего или отставшего узла растёт каждую секунду, и проверка
//! «номер вырос» (feed.stale_after_ms) его не ловит; возраст блока — ловит. Больше feed.max_block_age_ms — данные
//! устарели (StaleData), сделок нет; в журнал — одна запись на каждый переход «свежо ↔ устарело».
//! Настройка не задана — проверки нет и лишних вызовов к узлу нет (свежесть — только по росту номера, как раньше).
//! Ревью 122, пункт 4: свой узел (bsc.node_http_url или bsc.node_ws_url — петля, settings::value::is_loopback) без
//! порога — StaleData «задайте feed.max_block_age_ms» (свой узел может догонять сеть; нет данных — нет сделки).
//! Кто вызывает: node/heads.rs (fresh — проверка; run — время новой головы и записи в журнал; fetch_time — время
//! блока по запросу).
//! От чего зависит: ctx.rs, node/rpc.rs (call, call_unpaced), node/call.rs (block_tag), node/heads.rs (кэш времени).
//! Настройки: feed.max_block_age_ms [bsc_dex] (необязательная; обязательна при своём узле), bsc.node_http_url,
//! bsc.node_ws_url (петля ли адрес).
//!
//! Источник: eth_getBlockByNumber, поле timestamp — секунды Unix (hex) — execution-apis
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/schemas/block.yaml .
//! Время блока в секундах: измеренный возраст больше настоящего на долю секунды (0–1 с) — порог ставить с запасом.

use alloy_primitives::U64;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::value::is_loopback;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::call::block_tag;
use super::heads::Heads;
use super::rpc;

/// Имя настройки порога возраста головы.
pub const SETTING: &str = "feed.max_block_age_ms";

/// Поле timestamp блока из eth_getBlockByNumber.
#[derive(Deserialize)]
struct RawBlock {
    timestamp: U64,
}

/// Что делает: порог возраста головы из настроек площадки.
/// Возвращает: Some(мс) — проверка включена; None — настройка не задана (проверки нет).
/// Ошибки: Internal (ошибка хранилища).
pub fn max_age_ms(ctx: &Ctx) -> GatewayResult<Option<u64>> {
    match ctx.int(SETTING) {
        Ok(ms) => Ok(Some(ms)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e),
    }
}

/// Что делает: свой узел — адрес HTTP или WebSocket узла указывает на петлю (127.0.0.0/8, ::1, localhost).
/// Возвращает: да/нет (адрес не задан или не разбирается — нет). Ошибок нет.
pub fn own_node(ctx: &Ctx) -> bool {
    ["bsc.node_http_url", "bsc.node_ws_url"].iter().any(|k| {
        ctx.text(k)
            .ok()
            .and_then(|u| reqwest::Url::parse(&u).ok())
            .is_some_and(|u| is_loopback(&u))
    })
}

/// Что делает: проверка возраста действует — порог задан или узел свой (тогда без порога — StaleData). Ошибок нет.
pub fn active(ctx: &Ctx) -> bool {
    matches!(max_age_ms(ctx), Ok(Some(_))) || own_node(ctx)
}

/// Что делает: возраст блока по часам сервера.
/// Принимает: время блока (секунды Unix). Возвращает: мс от времени блока до сейчас (может быть < 0, если часы
/// сервера отстают). Ошибок нет.
pub fn age_ms(block_time_s: u64) -> i64 {
    Utc::now().timestamp_millis() - (block_time_s as i64).saturating_mul(1000)
}

/// Что делает: проверка возраста головы для fresh().
/// Принимает: окружение, последнее известное (номер, время блока). Возвращает: Ok — свежо или проверка выключена.
/// Ошибки: StaleData (время блока ещё не получено, блок старше порога, свой узел без порога), Internal (хранилище).
pub fn check(ctx: &Ctx, known: Option<(u64, u64)>) -> GatewayResult<()> {
    let Some(max) = max_age_ms(ctx)? else {
        if own_node(ctx) {
            let why = format!(
                "Узел BSC на этом сервере (адрес — петля): задайте {SETTING} — без порога возраста головы торговли нет"
            );
            return Err(ctx.err(ErrorCode::StaleData, why));
        }
        return Ok(());
    };
    let Some((number, t)) = known else {
        return Err(ctx.err(
            ErrorCode::StaleData,
            format!("Время последнего блока ещё не получено ({SETTING} задан)"),
        ));
    };
    let age = age_ms(t);
    if age > max as i64 {
        return Err(ctx.err(
            ErrorCode::StaleData,
            format!(
                "Блок {number} создан {age} мс назад — больше {SETTING} ({max}): узел отстаёт от сети или догоняет её"
            ),
        ));
    }
    Ok(())
}

/// Что делает: время блока с узла (eth_getBlockByNumber).
/// Принимает: окружение, номер блока, paced — через общую очередь темпа (node/pace.rs) или мимо неё (опрос головы:
/// от него зависит свежесть, как и eth_blockNumber). Возвращает: время блока (секунды Unix).
/// Ошибки: узел; BadResponse (узел не знает блок).
pub async fn fetch(ctx: &Ctx, number: u64, paced: bool) -> GatewayResult<u64> {
    let params = json!([block_tag(number), false]);
    let b: Option<RawBlock> = if paced {
        rpc::call(ctx, "eth_getBlockByNumber", params).await?
    } else {
        rpc::call_unpaced(ctx, "eth_getBlockByNumber", params).await?
    };
    b.map(|b| b.timestamp.to::<u64>()).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("узел BSC (HTTP) не знает блок {number}"),
        )
    })
}

/// Что делает: задан порог — время новой головы сразу с узла мимо очереди темпа и в кэш Heads. Сбой — время
/// остаётся от прошлого блока, голова стареет (StaleData по порогу). Не задан — ничего (лишних вызовов нет).
/// Принимает: головы, окружение, номер новой головы. Ошибок не возвращает.
pub async fn time_of_new_head(heads: &Heads, ctx: &Ctx, number: u64) {
    if !matches!(max_age_ms(ctx), Ok(Some(_))) {
        return;
    }
    if let Ok(t) = fetch(ctx, number, false).await {
        heads.remember_time(number, t);
    }
}

/// Состояние для записей в журнал: одна запись на переход «свежо ↔ устарело».
#[derive(Default)]
pub struct AgeLog {
    /// Последнее записанное: Some(true) — устарело, Some(false) — свежо, None — проверка выключена / ещё не было.
    stale: Option<bool>,
}

impl AgeLog {
    /// Что делает: пишет в журнал, если состояние сменилось.
    /// Принимает: окружение; результат check (None — проверка выключена: состояние сбрасывается без записи).
    /// Ошибок не возвращает.
    pub async fn step(&mut self, ctx: &Ctx, result: Option<&GatewayResult<()>>) {
        let Some(result) = result else {
            self.stale = None;
            return;
        };
        let now = result.is_err();
        if self.stale == Some(now) {
            return;
        }
        let first = self.stale.is_none();
        self.stale = Some(now);
        match result {
            Err(e) => {
                ctx.log(
                    Level::Warn,
                    "venue.head_age",
                    format!("Голова сети устарела: {}", e.message),
                    None,
                )
                .await
            }
            // Первая проверка и сразу свежо — не шумим; запись только после «устарело».
            Ok(()) if first => {}
            Ok(()) => {
                let msg = format!("Голова сети снова свежая (возраст блока не больше {SETTING})");
                ctx.log(Level::Info, "venue.head_age", msg, None).await;
            }
        }
    }
}
