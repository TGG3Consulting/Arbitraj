//! Файл: gateway/addr_create.rs
//! Что делает: решение 379, правки по ревью t165 — общая для адаптеров логика создания адреса депозита (Poloniex, Kraken):
//! - Б1: адрес после создания берётся ТОЛЬКО из повторного запроса списка адресов (GET), ответ создания (POST) лишь
//!   сверяется с ним (`confirm`); нет в GET или не совпал — отказ, в кэш не идёт, сделки нет;
//! - Н2, Н-А (ревью 2): неудача создания помнится по (монета, сеть) (`Creating`, gateway/addr_create_memo.rs):
//!   явный отказ биржи — на срок venue.networks_fresh_ms (не задана — до перезапуска), создание, отправленное без
//!   подтверждения списком (принято или итог неизвестен), — до перезапуска (`Failure`, `post_failure`); причина видна;
//! - Н4: одно создание за раз на (монета, сеть) (`Creating::enter`).
//!
//! Повтор POST на резервном адресе REST после таймаута (Н1) запрещён классом операции адаптера (Op::Create:
//! на резерв — только «запрос точно не отправлен», как ордер и вывод).
//! Кто вызывает: venues/poloniex/deposit_addr.rs, venues/kraken/deposit_addr.rs и deposit_addr_claim.rs (confirm),
//! тесты ниже.
//! От чего зависит: gateway/errors.rs, gateway/addr_create_memo.rs (Creating).
//! Настройки: нет своих (venue.networks_fresh_ms читает адаптер и передаёт сюда).

use std::time::{Duration, Instant};

use super::errors::{ErrorCode, GatewayError};

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

/// Ревью 2 t165 (Н-А): вид неудачи создания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Явный отказ биржи (код биржи, права, 429, запрос не ушёл) — повтор после срока venue.networks_fresh_ms.
    Refused,
    /// Создание отправлено и принято или итог неизвестен, а адреса в повторном списке нет (или адреса разные) — биржа,
    /// возможно, уже создала адрес: повторного создания нет до перезапуска процесса (GET идёт на каждом вызове).
    Sent,
}

/// Неудача создания и её вид — итог функции создания адаптера.
#[derive(Debug, Clone)]
pub struct Failed {
    /// В коробке — итог Result<_, Failed> остаётся маленьким (clippy result_large_err).
    pub err: Box<GatewayError>,
    pub kind: Failure,
}

/// Что делает: вид неудачи по коду ошибки запроса создания: «итог неизвестен» и таймаут — Sent, остальное — Refused.
/// Принимает: код. Возвращает: Failure. Ошибок нет.
pub fn post_failure(code: &ErrorCode) -> Failure {
    match code {
        ErrorCode::OutcomeUnknown | ErrorCode::Timeout => Failure::Sent,
        _ => Failure::Refused,
    }
}

/// Что делает: свежа ли запомненная неудача (Н2, Н-А). Принимает: когда запомнена, сейчас, срок (None — настройка не
/// задана: до перезапуска), вид. Возвращает: true — создание не повторять. Sent — всегда true (до перезапуска).
/// Ошибок нет.
pub fn fail_fresh(at: Instant, now: Instant, keep: Option<Duration>, kind: Failure) -> bool {
    kind == Failure::Sent || keep.is_none_or(|k| now.saturating_duration_since(at) < k)
}

/// Что делает: пояснение к запомненному отказу — когда создание будет повторено. Принимает: вид, срок.
/// Возвращает: текст. Ошибок нет.
pub fn retry_text(kind: Failure, keep: Option<Duration>) -> String {
    match (kind, keep) {
        (Failure::Sent, _) => {
            "создание отправлено, адрес в списке не появился — проверьте в кабинете биржи; \
                               повторно не создаю до перезапуска"
                .into()
        }
        (Failure::Refused, Some(d)) => format!(
            "создание не повторяется: повтор не раньше чем через {} мс (venue.networks_fresh_ms)",
            d.as_millis()
        ),
        (Failure::Refused, None) => {
            "создание не повторяется: до перезапуска — venue.networks_fresh_ms не задана".into()
        }
    }
}

pub use super::addr_create_memo::Creating;

#[cfg(test)]
mod tests {
    use super::*;

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
}
