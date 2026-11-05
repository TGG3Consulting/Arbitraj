//! Файл: api/usdt_transfer.rs
//! Что делает: «Перевести USDT» в интерфейсе (решение 176: интерфейс и Telegram равны) — списки площадок с USDT и
//! сводка перевода до отправки; сама отправка — POST /api/manual/command {"TransferUsdt": {...}} (та же команда ядра,
//! что у кнопки «Отправить» в Telegram, core/usdt_transfer.rs).
//!
//! GET  /api/usdt-transfer/sources → {sources: [{venue, wallet, key, label, usdt, error}]} — биржи и кошельки DEX
//!      (решение 132) со свободными USDT торгового счёта из памяти панели «Балансы» (не старше ui.balances_refresh_ms)
//! POST /api/usdt-transfer/quote {from, from_wallet, to, to_wallet, amount?} → {ok, quote?, error?} — сводка:
//!      можно вывести, комиссия и минимум вывода, сколько дойдёт; amount не задан — «Всё»
//!
//! Кто вызывает: api/server.rs; интерфейс ui/src/components/UsdtTransfer.svelte.
//! От чего зависит: api/state.rs, core/usdt_sources.rs, core/usdt_transfer_plan.rs.
//! Настройки: ui.balances_refresh_ms (возраст балансов), venue.quote_asset.

use axum::Json;
use axum::extract::State;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::core::usdt_sources;
use crate::core::usdt_transfer_plan as plan;

use super::state::AppState;

/// Площадки с USDT. Ошибок нет: сбой площадки — в её поле error.
pub async fn sources(State(st): State<AppState>) -> Json<Value> {
    let all = usdt_sources::all(&st.core, st.core.balances_age()).await;
    let list: Vec<Value> = all.iter().map(|s| s.to_json()).collect();
    Json(json!({ "sources": list }))
}

/// Тело сводки.
#[derive(Deserialize)]
pub struct QuoteBody {
    from: String,
    from_wallet: Option<String>,
    to: String,
    to_wallet: Option<String>,
    /// Сумма строкой-числом; нет — «Всё».
    amount: Option<String>,
}

/// Сводка перевода. Ошибок HTTP нет: отказ проверки — {ok: false, error}.
pub async fn quote(State(st): State<AppState>, Json(b): Json<QuoteBody>) -> Json<Value> {
    let res = async {
        let amount = match b.amount.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => Some(
                a.replace(',', ".")
                    .parse::<Decimal>()
                    .map_err(|_| format!("сумма «{a}» — не число"))?,
            ),
            None => None,
        };
        let f = plan::end(&st.core, &b.from, b.from_wallet.as_deref())?;
        let t = plan::end(&st.core, &b.to, b.to_wallet.as_deref())?;
        plan::quote(&st.core, &f, &t, amount).await
    }
    .await;
    Json(match res {
        Ok(q) => json!({ "ok": true, "quote": q.to_json() }),
        Err(e) => json!({ "ok": false, "error": e }),
    })
}
