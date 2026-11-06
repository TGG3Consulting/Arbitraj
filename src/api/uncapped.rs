//! Файл: api/uncapped.rs
//! Что делает: экран «Упущено из-за потолка» (решение 141) — итоги за период (исполненные сделки и не исполненные
//! кандидаты, таблица «потолок → прибыль», топ направлений, счётчики очереди), страница записей, одна запись (состав
//! затрат на оптимуме без потолка) и запись сделки по её id (карточка сделки).
//! Кто вызывает: интерфейс (ui/src/pages/Uncapped.svelte, ui/src/pages/Deals.svelte).
//! От чего зависит: api/state.rs, core (uncapped_view.rs), storage/uncapped_db.rs (Filter).
//! Настройки: нет (uncapped.* — экран «Настройки»). Период, сколько строк и топ — параметры запроса (задаёт зритель).
//!
//! GET /api/uncapped/summary?from=ISO&to=ISO&top=N — {off, deals, candidates, ladder, ladder_error, uncapped, top, stats}
//! GET /api/uncapped?limit=N&offset=M&from=ISO&to=ISO&kind=deal|candidate — {rows: [{id, body}], total}
//! GET /api/uncapped/{id}                — запись (JSON core/uncapped_types.rs Record); 404 — нет
//! GET /api/uncapped/deals               — {id сделки: {id, body}} последних записей всех сделок (экран «Сделки»)
//! GET /api/uncapped/deal/{deal_id}      — {id, body} последней записи сделки; 404 — не посчитана

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::core::uncapped_store::ts;
use crate::storage::uncapped_db::Filter;

use super::state::{ApiError, AppState};

/// Период.
#[derive(Deserialize)]
pub struct SumQ {
    from: Option<String>,
    to: Option<String>,
    top: usize,
}

/// Страница.
#[derive(Deserialize)]
pub struct ListQ {
    limit: usize,
    #[serde(default)]
    offset: usize,
    from: Option<String>,
    to: Option<String>,
    kind: Option<String>,
}

/// Время запроса (RFC 3339 с поясом) → вид столбца базы. Пусто — без границы. Ошибки: 400 — не время.
fn time(v: Option<String>) -> Result<Option<String>, ApiError> {
    match v.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|t| Some(ts(t.with_timezone(&Utc))))
            .map_err(|e| ApiError::bad(format!("время «{s}»: {e}"))),
    }
}

/// Итоги периода. Ошибки: 400 — неверное время; 500 — база.
pub async fn summary(
    State(st): State<AppState>,
    Query(q): Query<SumQ>,
) -> Result<Json<Value>, ApiError> {
    st.core
        .uncapped_summary(time(q.from)?, time(q.to)?, q.top)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// Страница записей. Ошибки: 400 — неверное время или вид; 500 — база.
pub async fn list(
    State(st): State<AppState>,
    Query(q): Query<ListQ>,
) -> Result<Json<Value>, ApiError> {
    let kind = match q.kind.as_deref() {
        None | Some("") => None,
        Some(k @ ("deal" | "candidate")) => Some(k.to_string()),
        Some(k) => {
            return Err(ApiError::bad(format!(
                "kind = «{k}»: только deal или candidate"
            )));
        }
    };
    let f = Filter {
        from: time(q.from)?,
        to: time(q.to)?,
        kind,
    };
    st.core
        .uncapped_list(f, q.limit, q.offset)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// Одна запись. Ошибки: 404 — нет; 500 — база.
pub async fn get(State(st): State<AppState>, Path(id): Path<i64>) -> Result<Json<Value>, ApiError> {
    match st.core.uncapped_get(id).await {
        Ok(Some(v)) => Ok(Json(v)),
        Ok(None) => Err(ApiError(StatusCode::NOT_FOUND, format!("записи {id} нет"))),
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// Запись сделки. Ошибки: 404 — не посчитана; 500 — база.
pub async fn deal(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    match st.core.uncapped_deal(&id).await {
        Ok(Some(v)) => Ok(Json(v)),
        Ok(None) => Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("для сделки {id} расчёта без потолка нет"),
        )),
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// Последние записи всех сделок одним ответом. Ошибки: 500 — база.
pub async fn deals(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    st.core
        .uncapped_deals()
        .await
        .map(Json)
        .map_err(ApiError::internal)
}
