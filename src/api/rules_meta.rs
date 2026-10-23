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

use crate::gateway::rules::{RuleAction as A, RuleScope};

use super::rules_codes::{CodeMeta, codes};

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
