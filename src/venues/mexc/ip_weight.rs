//! Файл: venues/mexc/ip_weight.rs
//! Что делает: свой ограничитель веса публичных REST-запросов MEXC (Тигран 09.11: согласовано; вопрос ревью 238 «у
//! MEXC rest::public своего ограничителя частоты нет — только пауза по Retry-After после 418/429»). Скользящее окно
//! документации (10 с): сумма веса публичных запросов бота за окно не больше venue.public_ip_weight_10s [mexc].
//! Приоритеты (как очередь узла BSC — gateway/urgency.rs, решение 61): запрос пути сделки — класс Trade, котировка
//! для исполнения (gateway/patience.rs exec_scope: снимок стакана для ордера, решение 238) или задача с терпением
//! ручной работы / продажи сделки (patience deadline_left) — НЕ ждёт ограничителя, но его вес учитывается в окне;
//! фоновый запрос (подтверждение тихой пары, справочник пар, делистинги, снимок окна) ждёт, пока его вес не поместится
//! в окно (не дольше окна). Так фон не задерживает котировку и ордер, а сам уступает им место в корзине.
//! Не задана venue.public_ip_weight_10s — ограничителя нет (как было; после 418/429 — пауза по Retry-After, reply.rs).
//! Корзина IP общая и с подписанными запросами с весом по IP (tradeFee 20, getall 10, account 10 …) — их ограничитель
//! не считает: настройка — доля корзины для публичных запросов (совет — docs/FIRST_RUN.md).
//! Кто вызывает: rest.rs (public — перед каждым публичным запросом).
//! От чего зависит: ctx.rs (настройка, площадка), gateway/urgency.rs, gateway/patience.rs, tokio.
//! Настройки: venue.public_ip_weight_10s [mexc] (необязательна).
//!
//! Официально, «Rate Limits» https://www.mexc.com/api-docs/spot-v3/introduction: «Endpoints that do not require an
//! access key are rate-limited per IP», «Each endpoint indicates whether limits are counted by IP or UID (account), and
//! the weight consumed per request», «IP-weighted endpoints share 300 weight per 10 seconds; UID-weighted endpoints
//! share 500 weight per 10 seconds»; «When you receive 429, you must stop sending requests … Continuing to exceed limits
//! after receiving 429 will result in an IP ban» («from a minimum of 2 minutes to a maximum of 3 days»).
//! Вес эндпоинтов (Market Data): Order Book «GET /api/v3/depth», «Weight(IP): 3»
//! (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/order-book; от limit вес не зависит — в документации
//! не сказано иного); Exchange Information «GET /api/v3/exchangeInfo», «Weight(IP): 25»
//! (https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information; один вес для запроса без
//! параметров, с symbol и с symbols). Других публичных путей адаптер не вызывает; новый путь без веса из документации —
//! отказ Internal (вес не угадывается).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::urgency::{self, Urgency};

use super::ctx::Ctx;

/// Окно корзины IP — из документации («300 weight per 10 seconds»).
const WINDOW: Duration = Duration::from_secs(10);

/// Настройка: вес за окно, который могут занять публичные запросы бота.
pub const SETTING: &str = "venue.public_ip_weight_10s";

/// Отправленные публичные запросы в окне: (когда, вес). Один адаптер MEXC на процесс (как пауза reply.rs).
static SENT: Mutex<VecDeque<(Instant, u64)>> = Mutex::new(VecDeque::new());

/// Вес публичного пути по документации (шапка). Принимает: путь. Возвращает: вес; None — веса в документации не
/// сверено (путь не используется адаптером). Ошибок нет.
pub fn weight(path: &str) -> Option<u64> {
    match path {
        "/api/v3/depth" => Some(3),
        "/api/v3/exchangeInfo" => Some(25),
        _ => None,
    }
}

/// Срочный ли запрос: путь сделки (Trade), котировка для исполнения или терпение ручной работы / продажи. Ошибок нет.
fn urgent() -> bool {
    urgency::current() == Urgency::Trade
        || crate::gateway::patience::is_exec()
        || crate::gateway::patience::deadline_left().is_some()
}

/// Место для публичного запроса в окне веса (порядок и приоритеты — шапка). Принимает: окружение, путь.
/// Возвращает: Ok — можно слать (вес уже учтён). Ошибки: Internal — веса пути нет в документации (шапка);
/// NotConfigured — вес запроса больше venue.public_ip_weight_10s (фоновый запрос не поместится никогда).
pub async fn slot(ctx: &Ctx, path: &str) -> GatewayResult<()> {
    let w = weight(path).ok_or_else(|| {
        ctx.tag(GatewayError::new(
            ErrorCode::Internal,
            None,
            format!("MEXC {path}: вес публичного запроса не сверен с документацией — запрос не отправлен"),
        ))
    })?;
    // Не задана — ограничителя нет (шапка).
    let Ok(budget) = ctx.int(SETTING) else {
        return Ok(());
    };
    let urgent = urgent();
    if !urgent && w > budget {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            SETTING,
            format!(
                "MEXC {path}: вес запроса {w} больше {SETTING} = {budget} — запрос не отправлен"
            ),
        )));
    }
    loop {
        let wait = match take(Instant::now(), w, budget, urgent) {
            None => return Ok(()),
            Some(d) => d,
        };
        tokio::time::sleep(wait).await;
    }
}

/// Занять место в окне. Принимает: момент, вес, бюджет окна, срочность. Возвращает: None — место занято (запрос
/// можно слать); Some — сколько ждать, пока освободится нужный вес (только фон). Ошибок нет.
pub fn take(now: Instant, w: u64, budget: u64, urgent: bool) -> Option<Duration> {
    let mut q = SENT.lock().unwrap_or_else(|p| p.into_inner());
    while q
        .front()
        .is_some_and(|(at, _)| now.duration_since(*at) >= WINDOW)
    {
        q.pop_front();
    }
    let used: u64 = q.iter().map(|(_, x)| x).sum();
    if urgent || used + w <= budget {
        q.push_back((now, w));
        return None;
    }
    // Сколько веса должно выйти из окна, и когда выйдет последний нужный запрос.
    let need = used + w - budget;
    let mut freed = 0;
    for (at, x) in q.iter() {
        freed += x;
        if freed >= need {
            return Some((*at + WINDOW).saturating_duration_since(now));
        }
    }
    Some(WINDOW)
}

/// Сколько веса публичных запросов сейчас в окне (для журнала и тестов). Ошибок нет.
pub fn used() -> u64 {
    let now = Instant::now();
    let q = SENT.lock().unwrap_or_else(|p| p.into_inner());
    q.iter()
        .filter(|(at, _)| now.duration_since(*at) < WINDOW)
        .map(|(_, x)| x)
        .sum()
}
