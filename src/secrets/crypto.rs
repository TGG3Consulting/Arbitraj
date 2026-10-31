//! Файл: secrets/crypto.rs
//! Что делает: шифрование и расшифровка содержимого файла секретов.
//! Ключ выводится из пароля функцией Argon2id (RFC 9106), содержимое шифруется
//! XChaCha20-Poly1305 (аутентифицированное шифрование: любая порча или неверный пароль
//! обнаруживаются). Заголовок файла (параметры Argon2id, соль, nonce) подписывается
//! вместе с содержимым как associated data — подменить параметры незаметно нельзя.
//! Кто вызывает: secrets/vault.rs.
//! От чего зависит: argon2, chacha20poly1305, getrandom, zeroize, base64, serde_json.
//! Настройки: параметры Argon2id приходят из security.kdf_* (при создании и пересохранении файла);
//! при открытии берутся из заголовка самого файла.
//!
//! Решение — docs/DECISIONS.md №27. Константы ниже — размеры, заданные самими алгоритмами.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Версия формата файла.
const FORMAT_VERSION: u32 = 1;
/// Длина ключа XChaCha20-Poly1305 — 32 байта (RFC 8439).
const KEY_LEN: usize = 32;
/// Длина nonce XChaCha20 — 24 байта (draft-irtf-cfrg-xchacha).
const NONCE_LEN: usize = 24;
/// Длина соли — 16 байт (RFC 9106, раздел 3.1: рекомендовано 128 бит).
const SALT_LEN: usize = 16;

/// Параметры Argon2id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Память, КиБ.
    pub memory_kib: u32,
    /// Число проходов.
    pub iterations: u32,
    /// Число потоков.
    pub parallelism: u32,
}

/// Заголовок файла (хранится открыто, защищён от подмены).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Header {
    version: u32,
    kdf: String,
    kdf_params: KdfParams,
    salt: String,
    cipher: String,
    nonce: String,
}

/// Файл целиком: заголовок + зашифрованное содержимое (base64).
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    header: Header,
    ciphertext: String,
}

/// Ошибка шифрования.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("Неверный пароль или файл секретов повреждён")]
    WrongPasswordOrCorrupted,
    #[error("Файл секретов в неизвестном формате: {0}")]
    BadFormat(String),
    #[error("Параметры Argon2id не подходят: {0}")]
    BadParams(String),
    #[error("Нет источника случайных чисел системы: {0}")]
    Random(String),
}

/// Шифрует содержимое.
/// Принимает: пароль, открытые данные, параметры Argon2id.
/// Возвращает: байты файла (JSON). Ошибки: BadParams, Random.
/// Каждый вызов — новая соль и новый nonce.
pub fn seal(password: &[u8], plain: &[u8], p: KdfParams) -> Result<Vec<u8>, CryptoError> {
    let salt = random::<SALT_LEN>()?;
    let nonce = random::<NONCE_LEN>()?;
    let header = Header {
        version: FORMAT_VERSION,
        kdf: "argon2id-v19".into(),
        kdf_params: p,
        salt: B64.encode(salt),
        cipher: "xchacha20poly1305".into(),
        nonce: B64.encode(nonce),
    };
    let aad = serde_json::to_vec(&header).map_err(|e| CryptoError::BadFormat(e.to_string()))?;
    let key = derive(password, &salt, p)?;
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let ct = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plain,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::BadFormat("ошибка шифрования".into()))?;
    let env = Envelope {
        header,
        ciphertext: B64.encode(ct),
    };
    serde_json::to_vec_pretty(&env).map_err(|e| CryptoError::BadFormat(e.to_string()))
}

/// Расшифровывает файл.
/// Принимает: пароль и байты файла. Возвращает: открытые данные (затираются в памяти при удалении).
/// Ошибки: WrongPasswordOrCorrupted, BadFormat, BadParams.
pub fn open(password: &[u8], file: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let env: Envelope =
        serde_json::from_slice(file).map_err(|e| CryptoError::BadFormat(e.to_string()))?;
    let h = &env.header;
    if h.version != FORMAT_VERSION || h.kdf != "argon2id-v19" || h.cipher != "xchacha20poly1305" {
        return Err(CryptoError::BadFormat(format!(
            "версия {} / {} / {}",
            h.version, h.kdf, h.cipher
        )));
    }
    let salt = decode::<SALT_LEN>(&h.salt)?;
    let nonce = decode::<NONCE_LEN>(&h.nonce)?;
    let ct = B64
        .decode(&env.ciphertext)
        .map_err(|e| CryptoError::BadFormat(e.to_string()))?;
    let aad = serde_json::to_vec(h).map_err(|e| CryptoError::BadFormat(e.to_string()))?;
    let key = derive(password, &salt, h.kdf_params)?;
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &ct,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::WrongPasswordOrCorrupted)
}

/// Проверяет, что блоб — файл секретов известного формата, НЕ расшифровывая его (пароль не нужен).
/// Принимает: байты файла. Возвращает: Ok(()) — конверт и заголовок узнаны; Err — не наш файл.
/// Для импорта бэкапа: ставим только настоящий зашифрованный контейнер, мусор отвергаем до записи.
/// Расшифровку и формат шифрования не трогает — читает лишь открытый заголовок (как open до derive).
pub fn check_format(file: &[u8]) -> Result<(), CryptoError> {
    let env: Envelope =
        serde_json::from_slice(file).map_err(|e| CryptoError::BadFormat(e.to_string()))?;
    let h = &env.header;
    if h.version != FORMAT_VERSION || h.kdf != "argon2id-v19" || h.cipher != "xchacha20poly1305" {
        return Err(CryptoError::BadFormat(format!(
            "версия {} / {} / {}",
            h.version, h.kdf, h.cipher
        )));
    }
    Ok(())
}

/// Выводит ключ из пароля (Argon2id, версия 0x13). Ключ затирается в памяти при удалении.
fn derive(
    password: &[u8],
    salt: &[u8],
    p: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, CryptoError> {
    let params = Params::new(p.memory_kib, p.iterations, p.parallelism, Some(KEY_LEN))
        .map_err(|e| CryptoError::BadParams(e.to_string()))?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, key.as_mut())
        .map_err(|e| CryptoError::BadParams(e.to_string()))?;
    Ok(key)
}

/// N случайных байт из системного источника (getrandom).
fn random<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| CryptoError::Random(e.to_string()))?;
    Ok(b)
}

/// base64 → массив ровно N байт. Ошибка BadFormat при другой длине.
fn decode<const N: usize>(s: &str) -> Result<[u8; N], CryptoError> {
    let v = B64
        .decode(s)
        .map_err(|e| CryptoError::BadFormat(e.to_string()))?;
    v.try_into()
        .map_err(|_| CryptoError::BadFormat("неверная длина соли или nonce".into()))
}
