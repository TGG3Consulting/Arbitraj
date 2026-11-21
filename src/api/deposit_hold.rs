//! Файл: api/deposit_hold.rs
//! Что делает: решение 413 (ревью t198, С2) — кнопка «Сбросить замер» удержания биржи при зачислении в окне монеты на
//! экране «Монеты»: замер (биржа, монета) удаляется, дальше — «не измерено» (в расчёте — налог перевода токена).
//! Журнал core.credit_fact пишет ядро (кто и что было), как изменения настроек.
//! Кто вызывает: интерфейс (ui/src/components/DepositHolds.svelte).
//! От чего зависит: api/state.rs, core/deposit_hold.rs, core/types.
//! Настройки: нет.
//!
//! POST /api/coins/deposit-hold/reset — {venue, asset: Asset} → {ok, details}

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use crate::core::types::{Asset, VenueId};
use crate::gateway::contract::ConnectionCheck;

use super::state::{ApiError, AppState};

/// Тело «Сбросить замер».
#[derive(Deserialize)]
pub struct ResetBody {
    venue: VenueId,
    asset: Asset,
}

/// «Сбросить замер» удержания (биржа, монета). Принимает: площадку и монету. Возвращает: {ok, details} — сброшен /
/// замера нет / сбой базы. Ошибки: нет (сбой — ok:false с причиной).
pub async fn reset(
    State(st): State<AppState>,
    Json(b): Json<ResetBody>,
) -> Result<Json<ConnectionCheck>, ApiError> {
    let who = "интерфейс: «Сбросить замер»";
    let res = match crate::core::deposit_hold::reset(&st.core, &b.venue, &b.asset, who).await {
        Ok(true) => ConnectionCheck {
            ok: true,
            details: "Замер сброшен — дальше «не измерено» до следующего однозначного зачисления"
                .into(),
        },
        Ok(false) => ConnectionCheck {
            ok: false,
            details: "Замера по этой бирже и монете нет".into(),
        },
        Err(e) => ConnectionCheck {
            ok: false,
            details: format!("Замер не сброшен: {e}"),
        },
    };
    Ok(Json(res))
}
