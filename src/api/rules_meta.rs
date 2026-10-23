//! Файл: api/rules_meta.rs
//! Что делает: справочник для формы правил шлюза — коды ошибок (событие правила),
//! действия с их параметрами и виды области. Это перечень вариантов типов из
//! gateway/errors.rs и gateway/rules.rs, а не настройки: значений и порогов здесь нет.
//! Кто вызывает: интерфейс (экран «Правила шлюза», выпадающие списки).
//! От чего зависит: gateway/errors.rs (ErrorCode), gateway/rules.rs (RuleAction, RuleScope).
//! Настройки: нет.
//!
//! GET /api/rules/meta — {codes:[{code,title}], actions:[{kind,title,params:[{name,title,unit}]}],
//!                        scopes:[{kind,title}]}
//!
//! Полнота перечня проверяется при сборке: в сторожах ниже исчерпывающий match.
//! Новый вариант в gateway/* → ошибка сборки здесь → добавить его в список рядом.

use axum::Json;
use serde::Serialize;

use crate::gateway::errors::ErrorCode as C;
use crate::gateway::rules::{RuleAction as A, RuleScope};

/// Код ошибки и его смысл по-русски (как в комментариях gateway/errors.rs).
#[derive(Serialize)]
pub struct CodeMeta {
    code: C,
    title: &'static str,
}

/// Параметр действия: имя поля в JSON правила, смысл, единица.
#[derive(Serialize)]
pub struct ParamMeta {
    name: &'static str,
    title: &'static str,
    unit: &'static str,
}

/// Действие: вид (как в JSON правила), смысл и параметры (пусто — без параметров).
#[derive(Serialize)]
pub struct ActionMeta {
    kind: &'static str,
    title: &'static str,
    params: &'static [ParamMeta],
}

/// Вид области правила (как поле kind в JSON правила) и смысл.
#[derive(Serialize)]
pub struct ScopeMeta {
    kind: &'static str,
    title: &'static str,
}

/// Весь справочник.
#[derive(Serialize)]
pub struct Meta {
    codes: Vec<CodeMeta>,
    actions: &'static [ActionMeta],
    scopes: &'static [ScopeMeta],
}

/// Короткая запись кода. Принимает: код и смысл. Возвращает: CodeMeta.
fn c(code: C, title: &'static str) -> CodeMeta {
    CodeMeta { code, title }
}

/// Все коды ошибок — порядок как в gateway/errors.rs.
fn codes() -> Vec<CodeMeta> {
    vec![
        c(
            C::InvalidCredentials,
            "Ключ или подпись не приняты площадкой",
        ),
        c(
            C::InsufficientPermissions,
            "У ключа нет нужного права (торговля, вывод)",
        ),
        c(C::IpNotAllowed, "IP сервера не в белом списке ключа"),
        c(C::Timeout, "Площадка не ответила за отведённое время"),
        c(C::Unreachable, "Нет связи с площадкой (DNS, TCP, TLS)"),
        c(
            C::InvalidAddress,
            "Адрес отклонён (не в белом списке, неверный формат)",
        ),
        c(C::RateLimited, "Превышен лимит запросов площадки"),
        c(C::NotConfigured, "Нужная настройка не задана в интерфейсе"),
        c(C::SecretsLocked, "Секреты не разблокированы паролем"),
        c(C::InsufficientBalance, "Не хватает средств"),
        c(C::OrderRejected, "Ордер отклонён площадкой"),
        c(C::OrderNotFilled, "Ордер отправлен, но не исполнился"),
        c(C::PartialFill, "Ордер исполнился частично"),
        c(C::WithdrawRejected, "Вывод отклонён площадкой"),
        c(
            C::DepositNotCredited,
            "Депозит не зачислен за отведённое время",
        ),
        c(C::SwapReverted, "Транзакция свопа в сети откатилась"),
        c(C::StaleData, "Данные устарели — торговать нельзя"),
        c(C::VenueNotConnected, "Адаптер площадки не подключён"),
        c(C::BadResponse, "Ответ площадки не удалось разобрать"),
        c(C::Internal, "Внутренняя ошибка нашего кода"),
    ]
}

/// Сторож полноты кодов: новый код в gateway/errors.rs → ошибка сборки → добавить в codes().
const _: fn(C) = |code| match code {
    C::InvalidCredentials | C::InsufficientPermissions | C::IpNotAllowed | C::Timeout => {}
    C::Unreachable | C::InvalidAddress | C::RateLimited | C::NotConfigured => {}
    C::SecretsLocked | C::InsufficientBalance | C::OrderRejected | C::OrderNotFilled => {}
    C::PartialFill | C::WithdrawRejected | C::DepositNotCredited | C::SwapReverted => {}
    C::StaleData | C::VenueNotConnected | C::BadResponse | C::Internal => {}
};

/// Все действия — порядок как в gateway/rules.rs.
static ACTIONS: &[ActionMeta] = &[
    ActionMeta {
        kind: "CancelNoRetry",
        title: "Отменить операцию и больше не пробовать",
        params: &[],
    },
    ActionMeta {
        kind: "Retry",
        title: "Повторить операцию",
        params: &[
            ParamMeta {
                name: "attempts",
                title: "Число повторов",
                unit: "раз",
            },
            ParamMeta {
                name: "pause_ms",
                title: "Пауза между повторами",
                unit: "мс",
            },
        ],
    },
    ActionMeta {
        kind: "StopVenue",
        title: "Остановить торговлю по площадке",
        params: &[],
    },
    ActionMeta {
        kind: "StopAll",
        title: "Остановить всю торговлю",
        params: &[],
    },
    ActionMeta {
        kind: "NotifyOnly",
        title: "Только уведомить",
        params: &[],
    },
];

/// Сторож полноты действий: новое действие или поле в gateway/rules.rs → ошибка сборки → обновить ACTIONS.
const _: fn(&A) = |a| match a {
    A::CancelNoRetry | A::StopVenue | A::StopAll | A::NotifyOnly => {}
    A::Retry {
        attempts: _,
        pause_ms: _,
    } => {}
};

/// Виды области правила.
static SCOPES: &[ScopeMeta] = &[
    ScopeMeta {
        kind: "All",
        title: "Общее — на все площадки",
    },
    ScopeMeta {
        kind: "Venue",
        title: "Одна площадка — сильнее общего",
    },
];

/// Сторож полноты областей: новый вид в gateway/rules.rs → ошибка сборки → обновить SCOPES.
const _: fn(&RuleScope) = |s| match s {
    RuleScope::All | RuleScope::Venue(_) => {}
};

/// Справочник для формы правил. Ошибок нет.
pub async fn meta() -> Json<Meta> {
    Json(Meta {
        codes: codes(),
        actions: ACTIONS,
        scopes: SCOPES,
    })
}
