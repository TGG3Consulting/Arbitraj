//! Файл: api/wallets_view.rs
//! Что делает: данные экрана «Маппинг кошельков» (решение 132): кошельки (id, имя, имя секрета ключа, адрес — от
//! адаптера, исполнитель, строки маппинга, незавершённые сделки), строки маппинга (каждая биржа реестра шлюза — без
//! перечисления в коде, «DEX ↔ DEX», «Пробы и сигналы») и предупреждения «деньги смешиваются». Ключей нет никогда.
//! Кто вызывает: api/wallets.rs (list, map).
//! От чего зависит: api/{state,wallets_dex}.rs, gateway (реестр), settings/accounts/, core/dex_account.rs.
//! Настройки: нет.

use serde_json::{Value, json};

use crate::gateway::contract::VenueKind;
use crate::settings::accounts::{ROW_DEX_DEX, ROW_PROBE};

use super::state::AppState;
use super::wallets_dex::{dex_for, dex_id};

/// Строки маппинга: (строка, вид — «cex» / «dex_dex» / «probe»). Биржи — из реестра шлюза по виду. Ошибок нет.
pub async fn rows(st: &AppState) -> Vec<(String, &'static str)> {
    let mut cex = Vec::new();
    for id in st.registry.ids().await {
        if let Ok(v) = st.registry.get(&id).await
            && v.kind() == VenueKind::Cex
        {
            cex.push(id.0);
        }
    }
    cex.sort();
    let mut out: Vec<(String, &'static str)> = cex.into_iter().map(|c| (c, "cex")).collect();
    out.push((ROW_DEX_DEX.to_string(), "dex_dex"));
    out.push((ROW_PROBE.to_string(), "probe"));
    out
}

/// Всё для экрана. Адрес кошелька — только при разблокированных секретах (иначе причина). Ошибок нет.
pub async fn view(st: &AppState) -> Value {
    let a = st.settings.accounts();
    let unlocked = st.vault.status().unlocked;
    let dex = dex_id(st).await.ok().map(|d| d.0);
    let mut wallets = Vec::new();
    for w in a.wallets() {
        let (address, why) = if !unlocked {
            (
                None,
                Some("секреты заблокированы — нужна разблокировка".to_string()),
            )
        } else {
            match dex_for(st, &w.id).await {
                Ok(v) => match v.wallet_address().await {
                    Ok(x) => (Some(x), None),
                    Err(e) => (None, Some(e.message)),
                },
                Err(e) => (None, Some(e.1)),
            }
        };
        let busy = crate::core::dex_account::open_deals_of(&st.core, &w.id);
        wallets.push(json!({
            "id": w.id,
            "name": w.name,
            "secret": w.secret,
            "executor": w.executor,
            "address": address,
            "address_error": why,
            "rows": a.rows_of(&w.id),
            "open_deals": busy,
            "legacy": a.legacy().as_deref() == Some(w.id.as_str()),
        }));
    }
    let rows: Vec<Value> = rows(st)
        .await
        .into_iter()
        .map(|(r, kind)| json!({ "row": r, "kind": kind, "wallet": a.row(&r) }))
        .collect();
    let shared: Vec<Value> = a
        .shared()
        .into_iter()
        .map(|(w, rows)| json!({ "wallet": w.id, "name": w.name, "rows": rows }))
        .collect();
    json!({
        "unlocked": unlocked,
        "dex": dex,
        "wallets": wallets,
        "rows": rows,
        "shared": shared,
    })
}
