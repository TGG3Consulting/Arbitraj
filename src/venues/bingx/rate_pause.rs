//! Файл: venues/bingx/rate_pause.rs
//! Что делает: решение 243 — пауза запросов REST BingX после HTTP 429 и 418 (урок 15: после «слишком часто» не слать
//! дальше). Раньше (факт кода): errors.rs распознавал 429/418 как RateLimited, но следующий запрос уходил сразу.
//! 429 — пауза только ТОГО эндпоинта (путь без параметров), на который пришёл ответ: фоновый 429 (стакан, справочник)
//! не останавливает постановку и опрос ордеров (как Bitget, решение 201). 418 (IP забанен) — пауза ВСЕЙ биржи (все
//! пути), журнал Error и событие уведомлений venue.rate_limited (MarketEvent::Notice → ядро → notify; одно на эпизод
//! бана — до первого ответа не 418).
//! Длительность: Retry-After из ответа (если BingX его пришлёт — транспорт gateway/http.rs читает заголовок), иначе
//! venue.rate_limit_pause_ms [bingx]. Нет ни того, ни другого — паузы нет (как было), но 429 пишется в журнал Warn, а
//! в тексте ошибки сказано, что настройка не задана. Во время паузы запрос не уходит: сразу RateLimited с причиной и
//! остатком; ручная работа ждёт конец паузы (gateway/patience.rs, решение 201) — rest.rs.
//! Кто вызывает: rest.rs (send — перед запросом и после ответа), mod.rs (set_out — канал событий из подписки на рынок).
//! От чего зависит: ctx.rs, gateway/errors.rs, gateway/events.rs, storage/journal.rs.
//! Настройки: venue.rate_limit_pause_ms [bingx].
//!
//! Официально: error-codes.md (https://github.com/BingX-API/api-ai-skills/blob/main/skills/references/error-codes.md):
//! «429 — Too Many Requests — Requests are too frequent, rate-limited by the system»; «418 — Continued access after
//! receiving 429, IP has been banned. Stop requests and wait». FAQ docs-v3 (md/faq.md): срок запрета не
//! указан; «Continuously sending frequent requests after receiving a 429 error response» — среди причин, по которым срок
//! запрета торговли может вырасти.
//! Заголовка Retry-After и времени до снятия бана в документации BingX нет; записанных ответов 429/418 в tests/data
//! нет — вид тела ответа неизвестен, пауза ставится по HTTP-статусу (тело разбирает rest.rs, как раньше).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};

use serde_json::json;
use tokio::sync::mpsc;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};

/// Событие уведомлений (notify/catalog.rs, группа «Площадки»): «Превышен лимит запросов».
pub const EVENT: &str = "venue.rate_limited";
/// Вид записи журнала.
const KIND: &str = "venue.rate_limit";

/// Паузы эндпоинтов после 429: путь → до какого момента (unix мс) не слать. Один адаптер BingX на процесс.
static PATHS: LazyLock<RwLock<HashMap<String, i64>>> = LazyLock::new(Default::default);
/// Пауза всей биржи после 418: до какого момента (unix мс). 0 — паузы нет.
static VENUE: AtomicI64 = AtomicI64::new(0);
/// Идёт эпизод бана (был 418, ответа не 418 ещё не было) — событие уже отправлено.
static BANNED: AtomicBool = AtomicBool::new(false);
/// Канал событий ядра (из подписки на рынок). Нет — событие не отправляется, журнал остаётся.
static OUT: LazyLock<Mutex<Option<mpsc::Sender<MarketEvent>>>> = LazyLock::new(Default::default);

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Запоминает канал событий ядра. Принимает: канал. Ошибок нет.
pub fn set_out(out: mpsc::Sender<MarketEvent>) {
    *OUT.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
}

/// Остаток паузы всей биржи (418), мс; ≤ 0 — паузы нет. Ошибок нет.
fn venue_left_ms() -> i64 {
    VENUE.load(Ordering::Relaxed) - now_ms()
}

/// Остаток паузы эндпоинта (429), мс; ≤ 0 — паузы нет. Принимает: путь. Ошибок нет.
fn path_left_ms(path: &str) -> i64 {
    PATHS
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(path)
        .copied()
        .unwrap_or(0)
        - now_ms()
}

/// Что делает: остаток паузы для запроса пути — большая из паузы биржи и паузы эндпоинта, мс (≤ 0 — паузы нет); для
/// терпения ручной работы (решение 201, gateway/patience.rs). Принимает: путь. Ошибок нет.
pub fn pause_left_ms(path: &str) -> i64 {
    venue_left_ms().max(path_left_ms(path))
}

/// Что делает: не пускает запрос во время паузы. Принимает: путь. Возвращает: Ok — можно слать.
/// Ошибки: RateLimited — какая пауза и сколько ещё ждать.
pub fn gate(path: &str) -> GatewayResult<()> {
    let venue = venue_left_ms();
    let text = if venue > 0 {
        format!(
            "BingX: пауза всей биржи после HTTP 418 (IP заблокирован за запросы после 429) — ещё {venue} мс запросы не \
             отправляются"
        )
    } else {
        let left = path_left_ms(path);
        if left <= 0 {
            return Ok(());
        }
        format!(
            "BingX {path}: пауза после HTTP 429 — ещё {left} мс запросы к этому эндпоинту не отправляются"
        )
    };
    Err(GatewayError::new(
        ErrorCode::RateLimited,
        Some(crate::core::types::VenueId(ID.to_string())),
        text,
    ))
}

/// Длительность паузы: Retry-After (секунды), иначе venue.rate_limit_pause_ms. Возвращает: (мс, откуда) или None —
/// ни того, ни другого. Ошибок нет.
fn length(ctx: &Ctx, retry_after_s: Option<u64>) -> Option<(u64, &'static str)> {
    if let Some(s) = retry_after_s {
        return Some((s.saturating_mul(1000), "Retry-After"));
    }
    ctx.int("venue.rate_limit_pause_ms")
        .ok()
        .map(|ms| (ms, "venue.rate_limit_pause_ms"))
}

/// Момент «сейчас + ms», unix мс. Ошибок нет.
fn until(ms: u64) -> i64 {
    now_ms().saturating_add(i64::try_from(ms).unwrap_or(i64::MAX))
}

/// Что делает: ответ получен — при 429 пауза эндпоинта, при 418 пауза биржи (журнал, событие); иной статус завершает
/// эпизод бана. Принимает: окружение, путь, HTTP-статус, Retry-After. Возвращает: пояснение для текста ошибки
/// (None — не 429/418). Ошибок нет.
pub async fn on_reply(
    ctx: &Ctx,
    path: &str,
    status: u16,
    retry_after_s: Option<u64>,
) -> Option<String> {
    match status {
        429 => Some(on_429(ctx, path, retry_after_s).await),
        418 => Some(on_418(ctx, path, retry_after_s).await),
        _ => {
            BANNED.store(false, Ordering::Relaxed);
            None
        }
    }
}

/// 429: пауза эндпоинта и запись Warn. Возвращает: пояснение. Ошибок нет.
async fn on_429(ctx: &Ctx, path: &str, retry_after_s: Option<u64>) -> String {
    let note = match length(ctx, retry_after_s) {
        Some((ms, from)) => {
            let mut m = PATHS.write().unwrap_or_else(|p| p.into_inner());
            let e = m.entry(path.to_string()).or_insert(0);
            *e = (*e).max(until(ms));
            format!("пауза эндпоинта {ms} мс ({from})")
        }
        None => {
            "venue.rate_limit_pause_ms [bingx] не задана и Retry-After нет — паузы нет, задайте её"
                .into()
        }
    };
    let msg = format!("BingX {path}: HTTP 429 «слишком частые запросы» — {note}");
    let data = json!({ "path": path, "retry_after_s": retry_after_s });
    let _ = ctx
        .journal
        .write(Level::Warn, KIND, Some(ID), msg, Some(data))
        .await;
    note
}

/// 418: пауза всей биржи, запись Error, событие (одно на эпизод). Возвращает: пояснение. Ошибок нет.
async fn on_418(ctx: &Ctx, path: &str, retry_after_s: Option<u64>) -> String {
    let note = match length(ctx, retry_after_s) {
        Some((ms, from)) => {
            VENUE.fetch_max(until(ms), Ordering::Relaxed);
            format!("пауза всей биржи {ms} мс ({from})")
        }
        None => {
            "venue.rate_limit_pause_ms [bingx] не задана и Retry-After нет — паузы нет, задайте её"
                .into()
        }
    };
    let text =
        format!("BingX: HTTP 418 — IP заблокирован за запросы после 429 (ответ на {path}); {note}");
    let first = !BANNED.swap(true, Ordering::Relaxed);
    let sent = first && {
        let out = OUT.lock().unwrap_or_else(|p| p.into_inner()).clone();
        out.is_some_and(|o| {
            o.try_send(MarketEvent::Notice {
                venue: ctx.venue(),
                code: EVENT.to_string(),
                text: text.clone(),
            })
            .is_ok()
        })
    };
    let tail = match (first, sent) {
        (false, _) => " (уведомление уже отправлено в этом эпизоде)",
        (true, true) => "",
        (true, false) => {
            " (уведомление не отправлено: нет канала событий ядра — подписка на рынок BingX не запущена)"
        }
    };
    let data = json!({ "path": path, "retry_after_s": retry_after_s });
    let _ = ctx
        .journal
        .write(
            Level::Error,
            KIND,
            Some(ID),
            format!("{text}{tail}"),
            Some(data),
        )
        .await;
    note
}

/// Добавляет пояснение паузы к тексту ошибки. Принимает: ошибку, пояснение (None — без изменений). Ошибок нет.
pub fn with_note(mut e: GatewayError, note: Option<String>) -> GatewayError {
    if let Some(n) = note {
        e.message = format!("{} — {n}", e.message);
    }
    e
}
