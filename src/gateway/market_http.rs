//! Общая пауза рыночного HTTP API после 429. Котировки и каталоги одного сервиса делят окно.
//! Отправка заявок, восстановление и собственный RPC вызывают свой транспорт.
use super::http::{self, Fail, Limits};
use reqwest::{RequestBuilder, StatusCode};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

type Key = (&'static str, String);
#[derive(Clone, Copy)]
struct Pause {
    since: Instant,
    duration: Duration,
}
impl Pause {
    fn left(self, now: Instant) -> Duration {
        self.duration.saturating_sub(now.saturating_duration_since(self.since))
    }
}
static PAUSES: LazyLock<Mutex<HashMap<Key, Pause>>> = LazyLock::new(Default::default);

#[derive(Debug)]
pub(crate) enum Error {
    Http(Fail),
    Held(Duration),
}
impl Error {
    pub(crate) fn held(&self) -> bool {
        matches!(self, Self::Held(_))
    }
    pub(crate) fn description(&self, limits: &Limits) -> String {
        match self {
            Self::Http(f) => f.text(limits),
            Self::Held(left) => format!("запрос не отправлен: пауза сервиса, осталось {} мс", left.as_millis()),
        }
    }
}

/// По принятой цели собранного запроса проверяет паузу до отправки. Тело запроса не копируется.
fn ready(service: &'static str, req: RequestBuilder) -> Result<(Key, RequestBuilder), Error> {
    let (client, built) = req.build_split();
    let built = built.map_err(|e| Error::Http(Fail::Send(e.without_url().to_string())))?;
    let key = (service, built.url().origin().ascii_serialization());
    let pauses = PAUSES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pause) = pauses.get(&key) {
        let left = pause.left(Instant::now());
        if !left.is_zero() {
            return Err(Error::Held(left));
        }
    }
    Ok((key, RequestBuilder::from_parts(client, built)))
}

fn pause(key: Key, status: StatusCode, retry_after_s: Option<u64>, limits: Limits) {
    if status != StatusCode::TOO_MANY_REQUESTS
        && !(status == StatusCode::SERVICE_UNAVAILABLE && retry_after_s.is_some())
    {
        return;
    }
    let duration = retry_after_s.map(Duration::from_secs).unwrap_or(limits.head);
    let mut pauses = PAUSES.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    pauses.retain(|_, p| !p.left(now).is_zero());
    if !duration.is_zero() && pauses.get(&key).is_none_or(|p| p.left(now) < duration) {
        pauses.insert(key, Pause { since: now, duration });
    }
}

pub(crate) async fn exchange(
    service: &'static str,
    req: RequestBuilder,
    limits: Limits,
) -> Result<(StatusCode, String), Error> {
    let (key, req) = ready(service, req)?;
    let reply = http::exchange_status::<true>(req, limits).await.map_err(Error::Http)?;
    pause(key, reply.status, reply.retry_after_s, limits);
    Ok((reply.status, reply.text))
}

/// KyberSwap читает всё тело и сохраняет ошибки чтения без журнала шагов.
/// Пауза вступает в силу по заголовкам, пока первый запрос ещё читает ответ.
pub(crate) async fn exchange_unmetered(
    service: &'static str,
    req: RequestBuilder,
    limits: Limits,
) -> Result<(StatusCode, String), Error> {
    let (key, req) = ready(service, req)?;
    let reply = http::full::<false>(req, limits, |status, retry| pause(key, status, retry, limits))
        .await.map_err(Error::Http)?;
    Ok((reply.status, reply.text))
}

#[cfg(test)]
#[path = "market_http_tests.rs"]
mod tests;
