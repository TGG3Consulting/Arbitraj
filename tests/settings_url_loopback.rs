//! Файл: tests/settings_url_loopback.rs
//! Что делает: проверка адресов настроек вида Url (решение 121, часть A1): https/wss — любой хост; http/ws — только
//! адрес-петля (свой узел BSC на том же сервере: http://127.0.0.1:8545, ws://127.0.0.1:8546).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::{catalog, value}. Моков нет: проверяются строки адресов, как их вводит человек.

use arb::settings::catalog;
use arb::settings::value::{Scope, validate};
use serde_json::json;

/// Проверка значения настройки bsc.node_http_url (вид Url, область «вся система»).
fn check(url: &str) -> Result<serde_json::Value, String> {
    let def = catalog::find("bsc.node_http_url").expect("bsc.node_http_url в каталоге");
    validate(def, &Scope::Global, &json!(url))
}

#[test]
fn loopback_plain_allowed() {
    for url in [
        "http://127.0.0.1:8545",
        "ws://127.0.0.1:8546",
        "http://localhost:8545",
        "ws://LOCALHOST:8546",
        "http://[::1]:8545",
        "ws://127.0.0.2:8546",
    ] {
        assert_eq!(check(url), Ok(json!(url)), "{url}");
    }
}

#[test]
fn secure_any_host_allowed() {
    for url in [
        "https://bsc-dataseed.bnbchain.org",
        "wss://bsc-rpc.publicnode.com",
        "https://127.0.0.1:8545",
    ] {
        assert_eq!(check(url), Ok(json!(url)), "{url}");
    }
}

#[test]
fn plain_remote_rejected() {
    for url in [
        "http://bsc-dataseed.bnbchain.org",
        "ws://10.0.0.5:8546",
        "http://localhost.example.com:8545",
        "http://0.0.0.0:8545",
    ] {
        let err = check(url).unwrap_err();
        assert!(
            err.contains("только для адреса на этом же сервере"),
            "{url}: {err}"
        );
    }
}

#[test]
fn other_scheme_rejected() {
    let err = check("ftp://127.0.0.1/x").unwrap_err();
    assert!(err.contains("«ftp»"), "{err}");
}
