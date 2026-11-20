//! Файл: api/wallets_chain.rs
//! Что делает: решение 432 (шаг 4 двух сетей) — выбор площадки DEX сети для экрана «Маппинг кошельков»: кошелёк один на
//! все сети, а баланс газа и USDT, «Проверить подключение» и исполнитель — у площадки DEX каждой сети (одна площадка на
//! сеть, решение 430). Параметр `?venue=<id площадки DEX>`; не указан — площадка DEX сети по умолчанию (как раньше).
//! Кто вызывает: api/wallets_dex.rs (balances, check, set_executor), api/wallets_view.rs (строки по сетям).
//! От чего зависит: api/{state, wallets_dex}.rs, gateway (реестр, вид площадки), core/chain.rs (dex.chain площадки).
//! Настройки: dex.chain (только чтение — подпись сети).

use serde::Deserialize;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;

use super::state::{ApiError, AppState};

/// Необязательная площадка DEX сети (`?venue=`).
#[derive(Deserialize, Default)]
pub struct VenueQuery {
    pub venue: Option<String>,
}

/// Что делает: площадки DEX реестра шлюза (по порядку реестра). Принимает: состояние API. Ошибок нет.
pub async fn dex_ids(st: &AppState) -> Vec<VenueId> {
    let mut dex = vec![];
    for id in st.registry.ids().await {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Dex
        {
            dex.push(id);
        }
    }
    dex
}

/// Что делает: площадка DEX запроса: указана — она (если это площадка DEX реестра), нет — DEX сети по умолчанию
/// (wallets_dex::dex_id, как до решения 432). Принимает: состояние API, id площадки или None.
/// Ошибки: 400 — указана не площадка DEX; 404 — как у dex_id.
pub async fn pick(st: &AppState, venue: Option<&str>) -> Result<VenueId, ApiError> {
    let Some(v) = venue.map(str::trim).filter(|v| !v.is_empty()) else {
        return super::wallets_dex::dex_id(st).await;
    };
    if dex_ids(st).await.iter().any(|d| d.0 == v) {
        Ok(VenueId(v.to_string()))
    } else {
        Err(ApiError::bad(format!("{v} — не площадка DEX")))
    }
}

/// Что делает: площадки DEX с их сетью (dex.chain; не задана — None) — для строк кошелька по сетям. Одна площадка DEX —
/// пусто (экран прежний). Принимает: состояние API. Ошибок нет.
pub async fn chains(st: &AppState) -> Vec<(VenueId, Option<String>)> {
    let dex = dex_ids(st).await;
    if dex.len() < 2 {
        return Vec::new();
    }
    dex.into_iter()
        .map(|v| {
            let c = crate::core::chain::chain_at(&st.settings, &v).ok().map(|c| c.0);
            (v, c)
        })
        .collect()
}
