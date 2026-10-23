//! Файл: api/settings.rs
//! Что делает: REST настроек — схема для интерфейса, текущие значения, изменение, история.
//! Кто вызывает: интерфейс (экран «Настройки», «Уведомления»).
//! От чего зависит: api/state.rs, settings, notify/catalog.rs.
//! Настройки: читает и меняет любые.
//!
//! GET  /api/settings/schema                 — каталог настроек и событий уведомлений
//! GET  /api/settings                        — все заданные значения
//! PUT  /api/settings                        — {name, scope, value} (value = null — очистить)
//! GET  /api/settings/history?limit=N[&name=] — история, новые первыми; limit обязателен

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::notify::catalog::EVENTS;
use crate::settings::catalog;
use crate::settings::live::Change;
use crate::settings::store::SetError;
use crate::settings::value::Scope;

use super::state::{ApiError, AppState, CHANGED_BY_UI};

/// Каталог настроек и событий. Ошибок нет.
pub async fn schema() -> Json<Value> {
    let settings: Vec<_> = catalog::all().collect();
    Json(json!({ "settings": settings, "events": EVENTS }))
}

/// Все заданные значения. Ошибок нет.
pub async fn values(State(st): State<AppState>) -> Json<Value> {
    let items: Vec<Value> = st
        .settings
        .all()
        .into_iter()
        .map(|(name, scope, value)| json!({ "name": name, "scope": scope, "value": value }))
        .collect();
    Json(Value::Array(items))
}

/// Тело запроса изменения.
#[derive(Deserialize)]
pub struct SetBody {
    name: String,
    scope: Scope,
    value: Option<Value>,
}

/// Меняет значение. Возвращает: изменение (старое и новое) или null — значение не изменилось.
/// Ошибки: 400 — нет такой настройки или неверное значение; 500 — база.
pub async fn set(
    State(st): State<AppState>,
    Json(b): Json<SetBody>,
) -> Result<Json<Option<Change>>, ApiError> {
    let value = b.value.filter(|v| !v.is_null());
    match st
        .settings
        .set(&b.name, b.scope, value, CHANGED_BY_UI)
        .await
    {
        Ok(c) => Ok(Json(c)),
        Err(SetError::Db(e)) => Err(ApiError::internal(e)),
        Err(e) => Err(ApiError::bad(e.to_string())),
    }
}

/// Параметры истории.
#[derive(Deserialize)]
pub struct HistoryQuery {
    limit: u32,
    name: Option<String>,
}

/// История изменений. Ошибки: 400 — не указан limit; 500 — база.
pub async fn history(
    State(st): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    st.settings
        .history(q.name, q.limit)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}
