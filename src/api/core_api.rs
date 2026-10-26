//! Файл: api/core_api.rs
//! Что делает: экраны «Живой экран», «Сделки», «Управление» — данные и команды ядра.
//! Кто вызывает: интерфейс.
//! От чего зависит: api/state.rs, core (Core::spreads, deals, control, set_mode, resume_venue,
//! release_quarantine, transits), core/risk.rs (Mode), storage/journal (через ядро).
//! Настройки: нет.
//!
//! GET  /api/spreads                               — [SpreadView]: спред направления, состояние, причина
//! GET  /api/candidates                            — последние расчёты кандидатов: [{direction, verdict, at}]
//! GET  /api/deals                                 — [Deal]: идущие и закрытые сделки, план и факт
//! GET  /api/transits                              — история времени в пути по монете и направлению
//! GET  /api/dry                                   — сухой режим: {state: on|nobudget|broken|off|unset, budget, reason}
//! GET  /api/control                               — Risk: режим системы, паузы площадок, карантины, итог дня
//! POST /api/control                               — {mode: "Running" | "Stopped" | "Paused"} → Risk
//! POST /api/control/venue/{id}/resume             — снять паузу площадки → Risk
//! POST /api/control/quarantine/{ticker}/release   — снять карантин монеты → Risk

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;

use crate::core::deal::Deal;
use crate::core::risk::{Mode, Risk};
use crate::core::signal::SpreadView;
use crate::core::types::{Asset, VenueId};

use super::state::AppState;

/// Строки «Живого экрана». Ошибок нет.
pub async fn spreads(State(st): State<AppState>) -> Json<Vec<SpreadView>> {
    Json(st.core.spreads())
}

/// Последние расчёты кандидатов (с причиной отказа). Ошибок нет.
pub async fn candidates(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(st.core.candidates())
}

/// Сделки, новые сверху. Ошибок нет.
pub async fn deals(State(st): State<AppState>) -> Json<Vec<Deal>> {
    Json(st.core.deals())
}

/// История времени в пути. Ошибок нет.
pub async fn transits(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(st.core.transits())
}

/// Сухой режим для плашки (решение 74): решает ядро (core/dry.rs status). Ошибок нет.
pub async fn dry(State(st): State<AppState>) -> Json<crate::core::dry::Status> {
    Json(crate::core::dry::status(&st.settings))
}

/// Состояние управления. Ошибок нет.
pub async fn control(State(st): State<AppState>) -> Json<Risk> {
    Json(st.core.control())
}

/// Тело команды режима.
#[derive(Deserialize)]
pub struct ModeBody {
    mode: Mode,
}

/// «Старт» / «Стоп» / «Пауза». Ядро пишет команду в журнал и шлёт system.started / system.stopped.
/// Ошибки: 400/422 — неверное тело.
pub async fn set_mode(State(st): State<AppState>, Json(b): Json<ModeBody>) -> Json<Risk> {
    st.core.set_mode(b.mode).await;
    Json(st.core.control())
}

/// Снять паузу площадки. Ошибок нет.
pub async fn resume_venue(State(st): State<AppState>, Path(id): Path<String>) -> Json<Risk> {
    st.core.resume_venue(&VenueId(id)).await;
    Json(st.core.control())
}

/// Снять карантин монеты (карантин ведётся по тикеру). Ошибок нет.
pub async fn release(State(st): State<AppState>, Path(ticker): Path<String>) -> Json<Risk> {
    let asset = Asset {
        ticker,
        bsc_contract: None,
    };
    st.core.release_quarantine(&asset).await;
    Json(st.core.control())
}
