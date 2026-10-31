//! Файл: secrets/vault.rs
//! Что делает: хранилище секретов в памяти и операции с ним: создать файл, разблокировать,
//! заблокировать, добавить или заменить секрет, удалить, сменить пароль.
//! Пароль нигде не хранится: каждое изменение требует ввести пароль заново (docs/SECURITY.md).
//! Тяжёлый Argon2id выполняется в отдельном потоке, чтобы не тормозить рыночные задачи.
//! Кто вызывает: api/secrets.rs (операции), notify и позже адаптеры (чтение через get).
//! От чего зависит: secrets/file.rs, secrets/crypto.rs, gateway/errors.rs.
//! Настройки: параметры Argon2id передаёт вызывающий (security.kdf_*).

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::crypto::{self, KdfParams};
use super::file::{self, Plain};

/// Состояние для интерфейса (без значений секретов).
#[derive(Debug, serde::Serialize)]
pub struct Status {
    /// Файл секретов уже создан.
    pub file_exists: bool,
    pub unlocked: bool,
    /// Имена введённых секретов (только при разблокировке).
    pub names: Vec<String>,
}

/// Секреты в памяти. None — заблокировано. Дешёво клонируется.
#[derive(Clone)]
pub struct Vault {
    path: PathBuf,
    keys: Arc<RwLock<Option<Plain>>>,
    /// Изменения файла — строго по одному.
    writer: Arc<Mutex<()>>,
}

impl Vault {
    /// Создаёт заблокированное хранилище. Принимает: путь к файлу секретов.
    pub fn locked(path: PathBuf) -> Self {
        Self {
            path,
            keys: Arc::new(RwLock::new(None)),
            writer: Arc::new(Mutex::new(())),
        }
    }

    /// Состояние для интерфейса. Ошибок нет.
    pub fn status(&self) -> Status {
        let keys = self.keys.read().unwrap_or_else(|p| p.into_inner());
        Status {
            file_exists: self.path.exists(),
            unlocked: keys.is_some(),
            names: keys
                .as_ref()
                .map(|k| k.keys().cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// Возвращает секрет по имени (значение затирается в памяти после использования).
    /// Ошибки: SecretsLocked — не разблокировано; NotConfigured — такой секрет не введён.
    pub fn get(&self, name: &str) -> Result<Zeroizing<String>, GatewayError> {
        let guard = self.keys.read().unwrap_or_else(|p| p.into_inner());
        let keys = guard.as_ref().ok_or_else(|| {
            GatewayError::new(
                ErrorCode::SecretsLocked,
                None,
                "Секреты не разблокированы — введите пароль в интерфейсе",
            )
        })?;
        keys.get(name).cloned().ok_or_else(|| {
            GatewayError::new(
                ErrorCode::NotConfigured,
                None,
                format!("Секрет {name} не введён — добавьте его в интерфейсе"),
            )
        })
    }

    /// Создаёт пустой файл секретов и разблокирует хранилище.
    /// Ошибки: файл уже есть; параметры Argon2id; запись.
    pub async fn create(&self, password: Zeroizing<String>, p: KdfParams) -> Result<(), String> {
        let _turn = self.writer.lock().await;
        if self.path.exists() {
            return Err("Файл секретов уже создан — разблокируйте его паролем".into());
        }
        self.write(password, Plain::new(), p).await
    }

    /// Разблокирует: расшифровывает файл паролем и держит секреты в памяти.
    /// Ошибки: нет файла, неверный пароль или повреждение.
    pub async fn unlock(&self, password: Zeroizing<String>) -> Result<(), String> {
        let path = self.path.clone();
        let plain = blocking(move || file::load(&path, password.as_bytes())).await?;
        *self.keys.write().unwrap_or_else(|p| p.into_inner()) = Some(plain);
        Ok(())
    }

    /// Блокирует: секреты удаляются из памяти (с затиранием). Ошибок нет.
    pub fn lock(&self) {
        *self.keys.write().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Добавляет или заменяет секрет (value = Some) либо удаляет (None).
    /// Принимает: пароль (проверяется расшифровкой файла), имя, значение, параметры Argon2id.
    /// Ошибки: неверный пароль, неверное имя, запись.
    pub async fn change(
        &self,
        password: Zeroizing<String>,
        name: &str,
        value: Option<Zeroizing<String>>,
        p: KdfParams,
    ) -> Result<(), String> {
        let ok_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_');
        if !ok_name {
            return Err(
                "Имя секрета: латиница в нижнем регистре, цифры, точка, подчёркивание".into(),
            );
        }
        let _turn = self.writer.lock().await;
        let mut plain = self.read_file(password.clone()).await?;
        match value {
            Some(v) => plain.insert(name.to_string(), v),
            None => plain.remove(name),
        };
        self.write(password, plain, p).await
    }

    /// Меняет пароль: расшифровка старым, шифрование новым (новая соль и nonce).
    /// Ошибки: неверный старый пароль, параметры, запись.
    pub async fn change_password(
        &self,
        old: Zeroizing<String>,
        new: Zeroizing<String>,
        p: KdfParams,
    ) -> Result<(), String> {
        let _turn = self.writer.lock().await;
        let plain = self.read_file(old).await?;
        self.write(new, plain, p).await
    }

    /// Отдаёт зашифрованный файл секретов КАК ЕСТЬ для экспорта/бэкапа — БЕЗ расшифровки.
    /// Пароль не нужен и хранилище можно не разблокировать: это просто байты файла.
    /// Возвращает: байты зашифрованного файла или None, если файла ещё нет.
    /// Ошибки: сбой чтения файла.
    pub async fn export_blob(&self) -> Result<Option<Vec<u8>>, String> {
        let path = self.path.clone();
        blocking(move || file::read_blob(&path)).await
    }

    /// Ставит зашифрованный файл секретов из бэкапа КАК ЕСТЬ (при импорте полного бэкапа) и блокирует
    /// хранилище: старые секреты в памяти (если были) больше не соответствуют новому файлу — их надо
    /// заново разблокировать паролем с исходной машины. Расшифровки нет: проверяется только формат
    /// конверта (crypto::check_format), затем атомарная запись поверх файла.
    /// Принимает: байты зашифрованного файла. Ошибки: не наш формат, сбой записи.
    pub async fn install_blob(&self, bytes: Vec<u8>) -> Result<(), String> {
        crypto::check_format(&bytes).map_err(|e| e.to_string())?;
        let _turn = self.writer.lock().await;
        let path = self.path.clone();
        blocking(move || file::write_blob(&path, &bytes)).await?;
        self.lock();
        Ok(())
    }

    /// Расшифровывает файл паролем (проверка пароля перед изменением).
    async fn read_file(&self, password: Zeroizing<String>) -> Result<Plain, String> {
        let path = self.path.clone();
        blocking(move || file::load(&path, password.as_bytes())).await
    }

    /// Шифрует и записывает файл, затем обновляет секреты в памяти (хранилище разблокировано).
    async fn write(
        &self,
        password: Zeroizing<String>,
        plain: Plain,
        p: KdfParams,
    ) -> Result<(), String> {
        let path = self.path.clone();
        let plain =
            blocking(move || file::save(&path, password.as_bytes(), &plain, p).map(|_| plain))
                .await?;
        *self.keys.write().unwrap_or_else(|p| p.into_inner()) = Some(plain);
        Ok(())
    }
}

/// Выполняет тяжёлую работу в отдельном потоке. Ошибка потока превращается в текст.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("Сбой потока: {e}"))?
}
