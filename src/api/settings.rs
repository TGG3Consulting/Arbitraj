//! Файл: api/settings.rs
//! Что делает: REST настроек — схема для интерфейса, текущие значения, изменение, история.
//! Кто вызывает: интерфейс (экран «Настройки», «Уведомления»).
//! От чего зависит: api/state.rs, api/dex_key.rs (площадки DEX шлюза), settings (dex_key.rs — проверка ключа «id@сеть»),
//! notify/catalog.rs.
//! Настройки: читает и меняет любые.
//!
//! GET  /api/settings/schema                 — каталог настроек и событий уведомлений
//! GET  /api/settings                        — все заданные значения
//! PUT  /api/settings                        — {name, scope, value} (value = null — очистить); решение 490: новый ключ
//!      области «DEX» вида «id@сеть» — только при площадке DEX этой сети не по умолчанию (settings/dex_key.rs check_net)
//! GET  /api/settings/history?limit=N[&name=][&scope=] — история, новые первыми; limit обязателен; scope — ключ области
//!      как в SQLite (решение 242: «venue_dir:bingx:cex_dex» — история одной строки «площадка · тип направления»)

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::notify::catalog::EVENTS;
use crate::settings::catalog;
use crate::settings::dex_key::check_net;
use crate::settings::live::Change;
use crate::settings::scope::check_dex_id;
use crate::settings::store::SetError;
use crate::settings::value::Scope;

use super::state::{ApiError, AppState, CHANGED_BY_UI};

/// Каталог настроек и событий. Ошибок нет.
pub async fn schema() -> Json<Value> {
    let settings: Vec<_> = catalog::all().collect();
    Json(
        json!({ "settings": settings, "events": EVENTS, "dex_profiles": crate::settings::evm_profiles::PROFILES }),
    )
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
    #[serde(default)]
    compare: Option<ExpectedValue>,
}

#[derive(Deserialize)]
struct ExpectedValue {
    value: Option<Value>,
}

/// Меняет значение. Возвращает: изменение (старое и новое) или null — значение не изменилось.
/// Ошибки: 400 — нет такой настройки, неверное значение или (решение 490) новый ключ «id@сеть» области «DEX» без
/// площадки DEX этой сети или с сетью по умолчанию; 500 — база.
pub async fn set(
    State(st): State<AppState>,
    Json(b): Json<SetBody>,
) -> Result<Json<Option<Change>>, ApiError> {
    let value = b.value.filter(|v| !v.is_null());
    // Решение 490: ключ «id@сеть» ввёл человек («Добавить DEX») — сеть должна быть сетью площадки DEX не по умолчанию.
    // Здесь, а не в Store::set: импорт настроек (api/settings_io.rs) восстанавливает уже созданные ключи и на свежей
    // базе идёт до перезапуска с dex.venues (площадки arb_dex ещё нет в шлюзе) — его проверка не должна отклонять.
    // Очистка не проверяется: неверный ключ можно удалить.
    if let (Scope::Dex(key), Some(_)) = (&b.scope, &value) {
        check_dex_id(key).map_err(ApiError::bad)?;
        let dexes = super::dex_key::dex_ids(&st).await;
        check_net(&st.settings, key, &dexes).map_err(ApiError::bad)?;
    }
    match st
        .settings
        .set_checked(&b.name,b.scope,value,CHANGED_BY_UI,b.compare.map(|c|c.value))
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
    scope: Option<String>,
}

/// История изменений. Ошибки: 400 — не указан limit; 500 — база.
pub async fn history(
    State(st): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    st.settings
        .history_in(q.name, q.scope, q.limit)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}
