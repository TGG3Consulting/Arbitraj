//! Файл: gateway/addr_create.rs
//! Что делает: решение 379, правки по ревью t165 — общая для адаптеров логика создания адреса депозита (Poloniex, MEXC):
//! - Б1: адрес после создания берётся ТОЛЬКО из повторного запроса списка адресов (GET), ответ создания (POST) лишь
//!   сверяется с ним (`confirm`); нет в GET или не совпал — отказ, в кэш не идёт, сделки нет;
//! - Н2: неудачное создание запоминается по (монета, сеть) — повторный POST не на каждый сигнал (`Fails`): срок —
//!   venue.networks_fresh_ms (сколько адаптер считает свежим справочник сетей и монет биржи; по смыслу ближе всего —
//!   у прогрева адресов повтор отказа тоже «до нового справочника / толчка»), не задана — до перезапуска процесса.
//!   Отказ при этом виден с причиной биржи (запомненный текст);
//! - Н4: одно создание за раз на (монета, сеть) (`Flights`): второй ждёт итога первого; ожидание — tokio-замок ключа,
//!   общий std-замок карты держится только на взятие Arc (без await под ним).
//!
//! Повтор POST на резервном адресе REST после таймаута (Н1) запрещён классом операции адаптера (Op::Create:
//! на резерв — только «запрос точно не отправлен», как ордер и вывод).
//! Кто вызывает: venues/poloniex/deposit_addr.rs, venues/mexc/deposit_addr.rs (через свои *_new.rs), тесты ниже.
//! От чего зависит: gateway/errors.rs, tokio (Mutex).
//! Настройки: нет своих (venue.networks_fresh_ms читает адаптер и передаёт сюда).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::errors::GatewayError;

/// Ключ: (тикер монеты на бирже, сеть) — как у кэша адресов адаптера.
pub type Key = (String, String);

/// Итог сверки создания (Б1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Адрес из GET — брать.
    Use(String),
    /// В GET адреса нет — создание не подтверждено.
    NotListed,
    /// Адрес GET не совпал с адресом из ответа POST.
    Mismatch,
}

/// Что делает: сверка адреса после создания. Принимает: адрес из ответа POST (None — итог POST неизвестен: таймаут
/// или ответ без адреса), адрес из повторного GET. Возвращает: Confirm. Адреса вида 0x… (EVM, BSC) сравниваются без
/// учёта регистра (регистр у них — только контрольная сумма), прочие — точно. Ошибок нет.
pub fn confirm(posted: Option<&str>, listed: Option<&str>) -> Confirm {
    let Some(listed) = listed else {
        return Confirm::NotListed;
    };
    let same = |p: &str| {
        let evm = p.starts_with("0x") && listed.starts_with("0x");
        if evm {
            p.eq_ignore_ascii_case(listed)
        } else {
            p == listed
        }
    };
    match posted {
        Some(p) if !same(p) => Confirm::Mismatch,
        _ => Confirm::Use(listed.to_string()),
    }
}

/// Что делает: свежа ли запомненная неудача (Н2). Принимает: когда запомнена, сейчас, срок (None — настройка не
/// задана: до перезапуска). Возвращает: true — POST не повторять. Ошибок нет.
pub fn fail_fresh(at: Instant, now: Instant, keep: Option<Duration>) -> bool {
    keep.is_none_or(|k| now.saturating_duration_since(at) < k)
}

/// Память неудачных созданий и замки «одно создание за раз». Дешёво клонируется (общая память).
#[derive(Clone, Default)]
pub struct Creating {
    fails: Arc<Mutex<HashMap<Key, (Instant, GatewayError)>>>,
    flights: Arc<Mutex<HashMap<Key, Arc<tokio::sync::Mutex<()>>>>>,
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

    /// Что делает: Н2 — запомненная свежая неудача по ключу. Принимает: ключ, срок (см. fail_fresh).
    /// Возвращает: ошибка прошлого создания (с причиной биржи) или None (не было, срок вышел — запись стирается).
    /// Ошибок нет.
    pub fn failed(&self, key: &Key, keep: Option<Duration>) -> Option<GatewayError> {
        let mut m = lk(&self.fails);
        let (at, e) = m.get(key)?;
        if fail_fresh(*at, Instant::now(), keep) {
            return Some(e.clone());
        }
        m.remove(key);
        None
    }

    /// Что делает: запомнить неудачу создания по ключу. Принимает: ключ, ошибку. Ошибок нет.
    pub fn fail(&self, key: &Key, e: &GatewayError) {
        lk(&self.fails).insert(key.clone(), (Instant::now(), e.clone()));
    }

    /// Что делает: адрес получен — неудача по ключу забывается. Принимает: ключ. Ошибок нет.
    pub fn ok(&self, key: &Key) {
        lk(&self.fails).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::errors::ErrorCode;

    fn key() -> Key {
        ("ESIM".into(), "BSC".into())
    }

    #[test]
    fn address_only_from_get() {
        // POST вернул адрес, а в GET его нет — не брать.
        assert_eq!(confirm(Some("0xab"), None), Confirm::NotListed);
        // Итог POST неизвестен, GET дал адрес — брать адрес GET.
        assert_eq!(confirm(None, Some("0xab")), Confirm::Use("0xab".into()));
        // Совпали (EVM — без учёта регистра) — брать адрес GET.
        assert_eq!(
            confirm(Some("0xAB"), Some("0xab")),
            Confirm::Use("0xab".into())
        );
    }

    #[test]
    fn post_and_get_mismatch_is_refusal() {
        assert_eq!(confirm(Some("0xab"), Some("0xcd")), Confirm::Mismatch);
        assert_eq!(
            confirm(Some("Ab"), Some("ab")),
            Confirm::Mismatch,
            "не EVM — точно"
        );
    }

    #[test]
    fn failed_create_not_retried_each_call() {
        let c = Creating::default();
        let e = GatewayError::new(ErrorCode::BadResponse, None, "отказ биржи");
        assert!(c.failed(&key(), None).is_none());
        c.fail(&key(), &e);
        // Не задан срок — до перезапуска; задан — пока не вышел.
        assert!(c.failed(&key(), None).is_some());
        assert!(c.failed(&key(), Some(Duration::from_secs(3600))).is_some());
        // Срок вышел — запись стирается, создание снова разрешено.
        assert!(c.failed(&key(), Some(Duration::ZERO)).is_none());
        assert!(c.failed(&key(), None).is_none());
        c.fail(&key(), &e);
        c.ok(&key());
        assert!(c.failed(&key(), None).is_none(), "адрес получен — забыто");
        let t = Instant::now();
        assert!(fail_fresh(t, t, Some(Duration::from_secs(1))));
        assert!(!fail_fresh(
            t,
            t + Duration::from_secs(1),
            Some(Duration::from_secs(1))
        ));
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
