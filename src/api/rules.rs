//! Файл: api/rules.rs
//! Что делает: REST правил шлюза — список, создание, изменение (выключение = enabled:false).
//! Каждое изменение пишется в журнал.
//! Кто вызывает: интерфейс (экран «Правила»).
//! От чего зависит: api/state.rs, storage/rules.rs, gateway/rules.rs, storage/journal.
//! Настройки: нет (правила — сами по себе данные из интерфейса).
//!
//! GET /api/rules        — все правила
//! POST /api/rules       — создать (id игнорируется)
//! PUT /api/rules/{id}   — заменить правило
//! Два включённых правила на одно событие и одну область запрещены. «Повторить» для «результат неизвестен»
//! (OutcomeUnknown) запрещено (решение 64).
//! Правило с условием пока не применяется (формат условия — этап 5); интерфейс должен это показывать.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::json;

use crate::gateway::rules::{Rule, RuleAction, retry_allowed};
use crate::storage::journal::Level;
use crate::storage::rules as store;

use super::state::{ApiError, AppState};

/// Все правила. Ошибки: 500 — база.
pub async fn list(State(st): State<AppState>) -> Result<Json<Vec<Rule>>, ApiError> {
    store::list(&st.db)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// Создаёт правило. Возвращает: правило с id. Ошибки: 500 — база.
pub async fn create(
    State(st): State<AppState>,
    Json(rule): Json<Rule>,
) -> Result<Json<Rule>, ApiError> {
    no_retry_where_forbidden(&rule)?;
    no_duplicate(&st, &rule).await?;
    let saved = store::create(&st.db, rule)
        .await
        .map_err(ApiError::internal)?;
    let _ = st
        .journal
        .write(
            Level::Info,
            "rules.created",
            None,
            format!("Создано правило №{}", saved.id),
            Some(json!(saved)),
        )
        .await;
    Ok(Json(saved))
}

/// Заменяет правило. Ошибки: 404 — нет такого правила; 500 — база.
pub async fn update(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(mut rule): Json<Rule>,
) -> Result<Json<Rule>, ApiError> {
    rule.id = id;
    no_retry_where_forbidden(&rule)?;
    no_duplicate(&st, &rule).await?;
    let old = store::list(&st.db)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|r| r.id == id);
    let found = store::update(&st.db, rule.clone())
        .await
        .map_err(ApiError::internal)?;
    if !found {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("Правила №{id} нет"),
        ));
    }
    let _ = st
        .journal
        .write(
            Level::Info,
            "rules.updated",
            None,
            format!("Изменено правило №{id}"),
            Some(json!({ "old": old, "new": rule })),
        )
        .await;
    Ok(Json(rule))
}

/// «Повторить» нельзя для кода, по которому повтор запрещён (gateway/rules.rs retry_allowed: «результат
/// неизвестен»). Ошибки: 400 — с причиной.
fn no_retry_where_forbidden(rule: &Rule) -> Result<(), ApiError> {
    if matches!(rule.action, RuleAction::Retry { .. }) && !retry_allowed(rule.event) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!(
                "Для {:?} повтор запрещён: операция могла исполниться — только сверка и ручная проверка",
                rule.event
            ),
        ));
    }
    Ok(())
}

/// Не даёт завести второе включённое правило на то же событие и ту же область:
/// иначе неясно, какое из них сработает.
/// Ошибки: 400 — такое правило уже есть; 500 — база.
async fn no_duplicate(st: &AppState, rule: &Rule) -> Result<(), ApiError> {
    if !rule.enabled {
        return Ok(());
    }
    let all = store::list(&st.db).await.map_err(ApiError::internal)?;
    match all
        .iter()
        .find(|r| r.id != rule.id && r.enabled && r.event == rule.event && r.scope == rule.scope)
    {
        Some(r) => Err(ApiError::bad(format!(
            "Уже есть включённое правило №{} на это событие и эту область — измените или выключите его",
            r.id
        ))),
        None => Ok(()),
    }
}
