//! Файл: api/secrets.rs
//! Что делает: REST секретов — состояние, создание файла, разблокировка, блокировка,
//! добавление/замена/удаление секрета, смена пароля. Значения секретов наружу не отдаются никогда.
//! Каждое действие (без значений) пишется в журнал; разблокировка — событие system.secrets_unlocked.
//! Кто вызывает: интерфейс (экран «Разблокировка» и поля ключей на экранах площадок и уведомлений).
//! От чего зависит: api/state.rs, secrets/vault.rs, settings (параметры Argon2id и длина пароля).
//! Настройки: security.kdf_memory_kib, security.kdf_iterations, security.kdf_parallelism, security.password_min_len.
//!
//! GET  /api/secrets/status
//! POST /api/secrets/create    {password}
//! POST /api/secrets/unlock    {password}
//! POST /api/secrets/lock
//! PUT  /api/secrets/{name}    {password, value}
//! DELETE /api/secrets/{name}  {password}
//! POST /api/secrets/password  {old, new}

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::secrets::crypto::KdfParams;
use crate::secrets::vault::Status;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::state::{ApiError, AppState};

/// Тело с паролем и, если нужно, значением.
#[derive(Deserialize)]
pub struct Body {
    password: Zeroizing<String>,
    value: Option<Zeroizing<String>>,
}

/// Тело смены пароля.
#[derive(Deserialize)]
pub struct PasswordBody {
    old: Zeroizing<String>,
    new: Zeroizing<String>,
}

/// Состояние: создан ли файл, разблокировано ли, имена секретов. Ошибок нет.
pub async fn status(State(st): State<AppState>) -> Json<Status> {
    Json(st.vault.status())
}

/// Создаёт файл секретов. Ошибки: 400 — не заданы настройки, короткий пароль, файл уже есть.
pub async fn create(
    State(st): State<AppState>,
    Json(b): Json<Body>,
) -> Result<Json<Status>, ApiError> {
    check_len(&st, &b.password)?;
    let p = kdf(&st)?;
    st.vault
        .create(b.password, p)
        .await
        .map_err(ApiError::bad)?;
    log(&st, Level::Info, "Создан файл секретов").await;
    Ok(Json(st.vault.status()))
}

/// Разблокирует секреты. Ошибки: 400 — неверный пароль (пишется в журнал), нет файла.
pub async fn unlock(
    State(st): State<AppState>,
    Json(b): Json<Body>,
) -> Result<Json<Status>, ApiError> {
    if let Err(e) = st.vault.unlock(b.password).await {
        log(
            &st,
            Level::Warn,
            &format!("Неудачная разблокировка секретов: {e}"),
        )
        .await;
        return Err(ApiError::bad(e));
    }
    log(&st, Level::Info, "Секреты разблокированы").await;
    let notifier = st.notifier.clone();
    tokio::spawn(async move {
        notifier
            .send("system.secrets_unlocked", None, "Секреты разблокированы")
            .await
    });
    Ok(Json(st.vault.status()))
}

/// Блокирует секреты (удаляет из памяти). Ошибок нет.
pub async fn lock(State(st): State<AppState>) -> Json<Status> {
    st.vault.lock();
    log(&st, Level::Info, "Секреты заблокированы").await;
    Json(st.vault.status())
}

/// Добавляет или заменяет секрет. Ошибки: 400 — нет значения, неверный пароль, имя, настройки.
pub async fn put(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<Body>,
) -> Result<Json<Status>, ApiError> {
    let value = b
        .value
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ApiError::bad("Пустое значение секрета"))?;
    let p = kdf(&st)?;
    st.vault
        .change(b.password, &name, Some(value), p)
        .await
        .map_err(ApiError::bad)?;
    log(
        &st,
        Level::Info,
        &format!("Секрет {name} добавлен или заменён"),
    )
    .await;
    Ok(Json(st.vault.status()))
}

/// Удаляет секрет. Ошибки: 400 — неверный пароль, настройки.
pub async fn delete(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<Body>,
) -> Result<Json<Status>, ApiError> {
    let p = kdf(&st)?;
    st.vault
        .change(b.password, &name, None, p)
        .await
        .map_err(ApiError::bad)?;
    log(&st, Level::Info, &format!("Секрет {name} удалён")).await;
    Ok(Json(st.vault.status()))
}

/// Меняет пароль. Ошибки: 400 — неверный старый пароль, короткий новый, настройки.
pub async fn password(
    State(st): State<AppState>,
    Json(b): Json<PasswordBody>,
) -> Result<Json<Status>, ApiError> {
    check_len(&st, &b.new)?;
    let p = kdf(&st)?;
    st.vault
        .change_password(b.old, b.new, p)
        .await
        .map_err(ApiError::bad)?;
    log(&st, Level::Info, "Пароль секретов изменён").await;
    Ok(Json(st.vault.status()))
}

/// Параметры Argon2id из настроек. Ошибка 400 — не заданы.
fn kdf(st: &AppState) -> Result<KdfParams, ApiError> {
    let get = |n| {
        st.settings
            .integer(n, At::default())
            .map_err(|e| ApiError::bad(e.message))
    };
    let to_u32 =
        |v: u64| u32::try_from(v).map_err(|_| ApiError::bad("Параметр Argon2id слишком большой"));
    Ok(KdfParams {
        memory_kib: to_u32(get("security.kdf_memory_kib")?)?,
        iterations: to_u32(get("security.kdf_iterations")?)?,
        parallelism: to_u32(get("security.kdf_parallelism")?)?,
    })
}

/// Проверка длины пароля по настройке. Ошибка 400 — не задана настройка или пароль короче.
fn check_len(st: &AppState, password: &str) -> Result<(), ApiError> {
    let min = st
        .settings
        .integer("security.password_min_len", At::default())
        .map_err(|e| ApiError::bad(e.message))?;
    if (password.chars().count() as u64) < min {
        return Err(ApiError::bad(format!("Пароль короче {min} символов")));
    }
    Ok(())
}

/// Запись в журнал без значений секретов.
async fn log(st: &AppState, level: Level, msg: &str) {
    let _ = st.journal.write(level, "secrets", None, msg, None).await;
}
