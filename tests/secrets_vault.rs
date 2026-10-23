//! Файл: tests/secrets_vault.rs
//! Что делает: проверяет шифрованное хранилище секретов на настоящем файле и настоящей криптографии:
//! создание, разблокировка верным и неверным паролем, добавление, удаление, смена пароля,
//! обнаружение подмены заголовка, отсутствие секретов в файле открытым текстом.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::secrets. Моков нет. Параметры Argon2id в тесте маленькие ради скорости.

use arb::secrets::Vault;
use arb::secrets::crypto::KdfParams;
use zeroize::Zeroizing;

const P: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

fn z(s: &str) -> Zeroizing<String> {
    Zeroizing::new(s.to_string())
}

fn fresh(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arb-sec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    arb::secrets::file::path_in(&dir)
}

#[tokio::test]
async fn full_cycle() {
    let path = fresh("cycle");
    let v = Vault::locked(path.clone());
    assert!(v.get("telegram.bot_token").is_err());
    v.create(z("пароль-1"), P).await.unwrap();
    assert!(
        v.create(z("пароль-1"), P).await.is_err(),
        "второй раз создать нельзя"
    );
    v.change(
        z("пароль-1"),
        "telegram.bot_token",
        Some(z("SECRET-VALUE-123")),
        P,
    )
    .await
    .unwrap();
    assert!(v.change(z("неверный"), "x", Some(z("y")), P).await.is_err());
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("SECRET-VALUE-123"),
        "в файле нет открытого текста"
    );

    let v2 = Vault::locked(path.clone());
    assert!(v2.unlock(z("неверный")).await.is_err());
    v2.unlock(z("пароль-1")).await.unwrap();
    assert_eq!(
        v2.get("telegram.bot_token").unwrap().as_str(),
        "SECRET-VALUE-123"
    );
    v2.lock();
    assert!(v2.get("telegram.bot_token").is_err());

    v.change_password(z("пароль-1"), z("пароль-2"), P)
        .await
        .unwrap();
    let v3 = Vault::locked(path.clone());
    assert!(v3.unlock(z("пароль-1")).await.is_err());
    v3.unlock(z("пароль-2")).await.unwrap();
    v3.change(z("пароль-2"), "telegram.bot_token", None, P)
        .await
        .unwrap();
    assert!(v3.get("telegram.bot_token").is_err());
}

#[tokio::test]
async fn tampered_header_is_detected() {
    let path = fresh("tamper");
    let v = Vault::locked(path.clone());
    v.create(z("пароль"), P).await.unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let tampered = raw.replace("\"iterations\": 1", "\"iterations\": 2");
    assert_ne!(raw, tampered);
    std::fs::write(&path, tampered).unwrap();
    assert!(Vault::locked(path).unlock(z("пароль")).await.is_err());
}
