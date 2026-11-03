//! Файл: api/dex_dry.rs
//! Что делает: «Сухая сделка DEX» для интерфейса (решение 121, C4): POST /api/dex/dry-swap → шлюз `dex_dry_swap`
//! (или `dex_dry_swap_executor` — режим «через исполнитель», решение 122) площадки DEX. Сборка и симуляция без отправки — у адаптера; api только находит площадку в реестре шлюза.
//! Кто вызывает: интерфейс (экран «Ручное управление», components/DexDrySwap.svelte).
//! От чего зависит: api/state.rs, gateway/registry.rs, gateway/contract.rs (VenueKind), core/types (DexDrySwap).
//! Настройки: нет.
//!
//! POST /api/dex/dry-swap {venue?, asset: {ticker, bsc_contract}, side: "Buy"|"Sell", usdt: "100", executor?: bool}
//! → DexDrySwap; executor = true — через исполнитель ArbExecutor с подставленным кодом (решение 122, часть X).
//! venue не задан — единственная площадка вида DEX в реестре (их несколько — 400 с просьбой указать).

use axum::Json;
use axum::extract::State;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::core::types::{Asset, DexDrySwap, Side, VenueId};
use crate::gateway::contract::VenueKind;

use super::state::{ApiError, AppState};

/// Тело запроса.
#[derive(Deserialize)]
pub struct DryReq {
    /// id площадки DEX; None — единственная DEX реестра.
    #[serde(default)]
    pub venue: Option<String>,
    pub asset: Asset,
    pub side: Side,
    /// Сумма USDT (строкой или числом, как Decimal).
    pub usdt: Decimal,
    /// Решение 122 (часть X): true — режим «через исполнитель» (любой маршрут шагами ArbExecutor, код подставлен в
    /// симуляцию — проверка до развёртывания); нет или false — как пойдёт сделка.
    #[serde(default)]
    pub executor: bool,
}

/// Сухая сделка DEX. Принимает: тело. Возвращает: итог адаптера (отказ — reason внутри, не HTTP-ошибка).
/// Ошибки: 400 — нет адреса монеты, сумма ≤ 0, DEX не одна и площадка не указана, площадка не DEX (NotApplicable);
/// 404 — площадка не подключена; 502 — сбой адаптера.
pub async fn dry_swap(
    State(st): State<AppState>,
    Json(b): Json<DryReq>,
) -> Result<Json<DexDrySwap>, ApiError> {
    if b.asset.bsc_contract.is_none() {
        return Err(ApiError::bad(
            "у монеты нет адреса контракта в BSC (asset.bsc_contract)",
        ));
    }
    if b.usdt <= Decimal::ZERO {
        return Err(ApiError::bad("сумма USDT должна быть больше 0"));
    }
    let id = match b.venue {
        Some(v) => VenueId(v),
        None => only_dex(&st).await?,
    };
    let venue = st.registry.get(&id).await.map_err(ApiError::gateway)?;
    let r = if b.executor {
        venue.dex_dry_swap_executor(b.asset, b.side, b.usdt).await
    } else {
        venue.dex_dry_swap(b.asset, b.side, b.usdt).await
    };
    r.map(Json).map_err(ApiError::gateway)
}

/// Единственная площадка вида DEX в реестре шлюза. Ошибки: 400 — ни одной или несколько.
async fn only_dex(st: &AppState) -> Result<VenueId, ApiError> {
    let mut found = Vec::new();
    for id in st.registry.ids().await {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Dex
        {
            found.push(id);
        }
    }
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(ApiError::bad("в реестре шлюза нет площадки DEX")),
        _ => {
            let list: Vec<&str> = found.iter().map(|v| v.0.as_str()).collect();
            Err(ApiError::bad(format!(
                "площадок DEX несколько ({}) — укажите venue",
                list.join(", ")
            )))
        }
    }
}
