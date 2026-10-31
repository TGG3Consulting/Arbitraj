//! Файл: api/settings_io.rs
//! Что делает: импорт/экспорт всех настроек одним файлом (TOML) — чтобы залить настройки разом,
//!   а не вводить по одной, и чтобы делать бэкап/перенос. Импорт применяет значения ТОЛЬКО через
//!   хранилище (Store::set, автор «import») — работают журнал изменений и обновление на лету.
//!   Секреты (пароль хранилища, ключи BingX, ключ кошелька, узла, индексатора) в файл НЕ входят:
//!   они в зашифрованном хранилище (docs/SECURITY.md), отдельный путь (api/secrets.rs).
//! Кто вызывает: интерфейс (экран «Импорт/экспорт настроек», ui/src/pages/SettingsIO.svelte).
//! От чего зависит: api/state.rs, api/settings_io_fmt.rs, settings (Store, Scope, SetError).
//! Настройки: своих нет; читает и задаёт все обычные настройки каталога.
//!
//! GET  /api/settings/export[?secrets=true] — TOML со всеми заданными настройками (имя+область+значение);
//!      с secrets=true — полный бэкап: в секцию [secrets] добавляется ЗАШИФРОВАННОЕ хранилище секретов
//!      как есть (base64), без расшифровки (docs/SECURITY.md). Для скачивания.
//! POST /api/settings/import — тело TOML (чистые настройки ИЛИ полный бэкап); проверяет и применяет
//!      настройки поштучно, а при наличии секции [secrets] ставит зашифрованное хранилище и блокирует его;
//!      JSON-отчёт {applied, failed, secrets_installed, secrets_error}.

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

use crate::settings::store::SetError;
use crate::settings::value::Scope;

use super::settings_io_fmt as fmt;
use super::state::{ApiError, AppState};

/// Кто меняет настройки при импорте (в журнал изменений). Отличает импорт от правок через форму.
const AUTHOR: &str = "import";

/// Применённая запись: имя и область (как в файле).
#[derive(Serialize)]
pub struct Applied {
    pub name: String,
    pub scope: String,
}

/// Отклонённая запись: имя, область и причина по-русски.
#[derive(Serialize)]
pub struct Failed {
    pub name: String,
    pub scope: String,
    pub reason: String,
}

/// Отчёт импорта: что применено, что отклонено и установилось ли зашифрованное хранилище секретов.
#[derive(Serialize, Default)]
pub struct Report {
    pub applied: Vec<Applied>,
    pub failed: Vec<Failed>,
    /// true — в файле была секция [secrets] и зашифрованное хранилище установлено (его надо разблокировать
    /// исходным паролем). false — секции не было или установить не удалось (см. secrets_error).
    pub secrets_installed: bool,
    /// Причина, если секция [secrets] была, но установить не удалось (не наш формат, сбой записи).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets_error: Option<String>,
}

impl Report {
    /// Запоминает применённую запись.
    fn ok(&mut self, name: &str, scope: &str) {
        self.applied.push(Applied {
            name: name.to_string(),
            scope: scope.to_string(),
        });
    }

    /// Запоминает отклонённую запись с причиной.
    fn fail(&mut self, name: &str, scope: &str, reason: impl Into<String>) {
        self.failed.push(Failed {
            name: name.to_string(),
            scope: scope.to_string(),
            reason: reason.into(),
        });
    }
}

/// Параметры экспорта: secrets=true — полный бэкап с зашифрованным хранилищем секретов.
#[derive(Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    secrets: bool,
}

/// Экспорт: отдаёт TOML со всеми заданными настройками на скачивание (имя+область+значение,
/// в шапке — версия формата). Порядок стабильный (по имени, затем по области) — файлы удобно сравнивать.
/// С secrets=true — полный бэкап: добавляет секцию [secrets] с ЗАШИФРОВАННЫМ хранилищем секретов как есть
/// (base64, без расшифровки); имя файла — arb-backup.toml, иначе arb-settings.toml.
/// Ошибки: 400 — secrets=true, но файла секретов ещё нет (бэкапить нечего); 500 — значение не представить
/// в TOML (повреждённая строка), сбой чтения файла секретов или сериализации.
pub async fn export(
    State(st): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let mut items = st.settings.all();
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.to_key().cmp(&b.1.to_key())));
    let mut setting = Vec::with_capacity(items.len());
    for (name, scope, value) in items {
        let v =
            fmt::json_to_toml(&value).map_err(|e| ApiError::internal(format!("{name}: {e}")))?;
        setting.push(fmt::Entry {
            name,
            scope: scope.to_key(),
            value: v,
        });
    }
    // Полный бэкап: зашифрованный файл секретов как есть (расшифровки нет). Нет файла — честная ошибка.
    let secrets = if q.secrets {
        match st.vault.export_blob().await.map_err(ApiError::internal)? {
            Some(blob) => Some(fmt::Secrets {
                blob: B64.encode(blob),
            }),
            None => {
                return Err(ApiError::bad(
                    "Файла секретов ещё нет — сначала создайте хранилище секретов",
                ));
            }
        }
    } else {
        None
    };
    let file = fmt::File {
        version: fmt::FORMAT_VERSION,
        setting,
        secrets,
    };
    let body = toml::to_string_pretty(&file).map_err(ApiError::internal)?;
    let name = if q.secrets {
        "arb-backup.toml"
    } else {
        "arb-settings.toml"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/toml; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(Body::from(body))
        .map_err(ApiError::internal)
}

/// Импорт: принимает TOML, проверяет каждую запись по схеме и применяет через Store::set (автор «import»):
/// работают журнал изменений и обновление на лету. Негодные записи не применяются и попадают в отчёт
/// с причиной. Применяет поштучно; возвращает {applied, failed}.
/// Ошибки: 400 — тело не разобралось как TOML; 500 — сбой базы при записи (останавливает импорт).
pub async fn import(State(st): State<AppState>, body: String) -> Result<Json<Report>, ApiError> {
    let file: fmt::File =
        toml::from_str(&body).map_err(|e| ApiError::bad(format!("Не разобрать TOML: {e}")))?;
    let mut report = Report::default();
    for e in file.setting {
        let scope = match Scope::from_key(&e.scope) {
            Some(s) => s,
            None => {
                report.fail(
                    &e.name,
                    &e.scope,
                    format!("неизвестная область «{}»", e.scope),
                );
                continue;
            }
        };
        let value = match fmt::toml_to_json(&e.value) {
            Ok(v) => v,
            Err(reason) => {
                report.fail(&e.name, &e.scope, reason);
                continue;
            }
        };
        match st.settings.set(&e.name, scope, Some(value), AUTHOR).await {
            Ok(_) => report.ok(&e.name, &e.scope),
            // Сбой базы — не вина записи: останавливаем импорт и сообщаем как внутреннюю ошибку.
            Err(SetError::Db(err)) => return Err(ApiError::internal(err)),
            Err(err) => report.fail(&e.name, &e.scope, err.to_string()),
        }
    }
    // Полный бэкап: ставим зашифрованное хранилище секретов как есть (расшифровки нет). Хранилище при этом
    // блокируется — пользователь разблокирует его исходным паролем. Ошибка установки — в отчёт, не 500.
    if let Some(s) = file.secrets {
        match B64.decode(s.blob.trim()) {
            Ok(bytes) => match st.vault.install_blob(bytes).await {
                Ok(()) => {
                    report.secrets_installed = true;
                    // Приём кнопок Telegram начинает заново с актуальным токеном (notify/updates.rs, wake).
                    st.secrets_wake.notify_one();
                }
                Err(err) => report.secrets_error = Some(err),
            },
            Err(err) => {
                report.secrets_error = Some(format!("секция секретов не в base64: {err}"));
            }
        }
    }
    Ok(Json(report))
}
