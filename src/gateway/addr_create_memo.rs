//! Файл: gateway/addr_create_memo.rs
//! Что делает: решение 379, ревью t165 Н2, Н4 и ревью 2 Н-А — память неудачных созданий адреса депозита по (монета,
//! сеть) и замки «одно создание за раз» (`Creating`). Отказ биржи помнится на срок venue.networks_fresh_ms, создание,
//! отправленное без подтверждения списком, — до перезапуска (gateway/addr_create.rs: Failure, fail_fresh, retry_text).
//! Ожидание очереди — tokio-замок ключа; общий std-замок карты держится только на взятие Arc (без await под ним).
//! Кто вызывает: venues/poloniex/deposit_addr.rs, venues/kraken/deposit_addr.rs (через addr_create::Creating), тесты.
//! От чего зависит: gateway/addr_create.rs, gateway/errors.rs, tokio (Mutex).
//! Настройки: нет своих (срок передаёт адаптер).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::addr_create::{Failed, Failure, Key, fail_fresh, retry_text};
use super::errors::GatewayError;

/// Запомненная неудача: когда, ошибка, вид.
struct Memo {
    at: Instant,
    err: Box<GatewayError>,
    kind: Failure,
}

/// Карта неудач по ключу.
type Fails = HashMap<Key, Memo>;
/// Карта замков «одно создание за раз» по ключу.
type Flights = HashMap<Key, Arc<tokio::sync::Mutex<()>>>;

/// Память неудачных созданий и замки «одно создание за раз». Дешёво клонируется (общая память).
#[derive(Clone, Default)]
pub struct Creating {
    fails: Arc<Mutex<Fails>>,
    flights: Arc<Mutex<Flights>>,
}

/// Std-замок без паники на отравлении. Ошибок нет.
fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Creating {
    /// Что делает: Н4 — ждёт своей очереди на создание по ключу. Принимает: ключ. Возвращает: пропуск (пока он жив,
    /// другой вызов с тем же ключом ждёт). Ошибок нет.
    pub async fn enter(&self, key: &Key) -> tokio::sync::OwnedMutexGuard<()> {
        let m = lk(&self.flights).entry(key.clone()).or_default().clone();
        m.lock_owned().await
    }

    /// Что делает: Н2, Н-А — запомненная свежая неудача по ключу. Принимает: ключ, срок (см. fail_fresh).
    /// Возвращает: ошибка прошлого создания (причина биржи и retry_text) или None (не было, срок отказа вышел — запись
    /// стирается). Ошибок нет.
    pub fn failed(&self, key: &Key, keep: Option<Duration>) -> Option<GatewayError> {
        let mut m = lk(&self.fails);
        let f = m.get(key)?;
        if fail_fresh(f.at, Instant::now(), keep, f.kind) {
            let mut e = (*f.err).clone();
            e.message = format!("{} ({})", e.message, retry_text(f.kind, keep));
            return Some(e);
        }
        m.remove(key);
        None
    }

    /// Что делает: запомнить неудачу создания по ключу. Принимает: ключ, неудачу. Возвращает: true — запись новая
    /// (для одной записи журнала Warn у Sent). Ошибок нет.
    pub fn fail(&self, key: &Key, f: &Failed) -> bool {
        let memo = Memo {
            at: Instant::now(),
            err: f.err.clone(),
            kind: f.kind,
        };
        lk(&self.fails).insert(key.clone(), memo).is_none()
    }

    /// Что делает: адрес получен — неудача по ключу забывается. Принимает: ключ. Ошибок нет.
    pub fn ok(&self, key: &Key) {
        lk(&self.fails).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::addr_create::post_failure;
    use crate::gateway::errors::ErrorCode;

    fn key() -> Key {
        ("ESIM".into(), "BSC".into())
    }

    #[test]
    fn failed_create_not_retried_each_call() {
        let c = Creating::default();
        let refused = Failed {
            err: Box::new(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "отказ биржи",
            )),
            kind: Failure::Refused,
        };
        assert!(c.failed(&key(), None).is_none());
        assert!(c.fail(&key(), &refused), "первая запись");
        // Не задан срок — до перезапуска; задан — пока не вышел.
        assert!(c.failed(&key(), None).is_some());
        assert!(c.failed(&key(), Some(Duration::from_secs(3600))).is_some());
        // Срок вышел — запись стирается, создание снова разрешено.
        assert!(c.failed(&key(), Some(Duration::ZERO)).is_none());
        assert!(c.failed(&key(), None).is_none());
        c.fail(&key(), &refused);
        c.ok(&key());
        assert!(c.failed(&key(), None).is_none(), "адрес получен — забыто");
        let t = Instant::now();
        let one = Some(Duration::from_secs(1));
        assert!(fail_fresh(t, t, one, Failure::Refused));
        assert!(!fail_fresh(
            t,
            t + Duration::from_secs(1),
            one,
            Failure::Refused
        ));
    }

    /// Ревью 2 Н-А: создание отправлено (принято или итог неизвестен), а адреса в списке нет — до перезапуска.
    #[test]
    fn sent_create_not_retried_until_restart() {
        let c = Creating::default();
        let sent = Failed {
            err: Box::new(GatewayError::new(
                ErrorCode::InvalidAddress,
                None,
                "адреса нет в списке",
            )),
            kind: Failure::Sent,
        };
        assert!(c.fail(&key(), &sent));
        assert!(
            !c.fail(&key(), &sent),
            "повторная запись — не новая (Warn один раз)"
        );
        let e = c
            .failed(&key(), Some(Duration::ZERO))
            .expect("срок отказа на Sent не действует");
        assert!(e.message.contains("повторно не создаю до перезапуска"));
        let t = Instant::now();
        assert!(fail_fresh(
            t,
            t + Duration::from_secs(3600),
            Some(Duration::ZERO),
            Failure::Sent
        ));
        assert_eq!(post_failure(&ErrorCode::OutcomeUnknown), Failure::Sent);
        assert_eq!(post_failure(&ErrorCode::Timeout), Failure::Sent);
        assert_eq!(post_failure(&ErrorCode::RateLimited), Failure::Refused);
        assert_eq!(post_failure(&ErrorCode::BadResponse), Failure::Refused);
    }

    #[tokio::test]
    async fn one_create_at_a_time() {
        let c = Creating::default();
        let first = c.enter(&key()).await;
        // Тот же ключ — ждёт, пока жив первый пропуск.
        let waiting = tokio::time::timeout(Duration::from_millis(50), c.enter(&key())).await;
        assert!(waiting.is_err(), "второй ждёт итога первого");
        // Другой ключ — не ждёт.
        let other = ("BANK".to_string(), "BSC".to_string());
        let _o = c.enter(&other).await;
        drop(first);
        let _second = c.enter(&key()).await;
    }
}
