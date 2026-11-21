//! Файл: api/delist.rs
//! Что делает: делистинги бирж для интерфейса (решение 175) — по каждой бирже последний ответ (пары в делистинге или
//! «неизвестно» с причиной), время ответа и последняя ошибка запроса. Экран «Площадки» (блок «Делистинги»), отметки
//! «делистинг на X: дата» на «Монетах» и «Живом экране» (кандидаты).
//! Кто вызывает: интерфейс (GET /api/delistings).
//! От чего зависит: api/state.rs, core (Core::delistings, core/delist.rs VenueDelist).
//! Настройки: нет (delist.refresh_s, delist.block_hours — экран «Настройки» / «Площадки»).

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::core::delist::VenueDelist;

use super::state::AppState;

/// Строка ответа: площадка и что о ней известно.
#[derive(Serialize)]
pub struct Row {
    pub venue: String,
    #[serde(flatten)]
    pub state: VenueDelist,
}

/// GET /api/delistings → [{venue, got: {items: [DelistItem], unknown?} | null, at | null, error | null}]. Ошибок нет.
pub async fn list(State(st): State<AppState>) -> Json<Vec<Row>> {
    let rows = st
        .core
        .delistings()
        .into_iter()
        .map(|(v, state)| Row { venue: v.0, state })
        .collect();
    Json(rows)
}
