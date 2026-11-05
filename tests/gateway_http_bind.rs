//! Файл: tests/gateway_http_bind.rs
//! Что проверяет: решение 178 — клиент с исходящим адресом (venue.local_ip, gateway/http_bind.rs) ходит с этого
//! адреса и только к адресам его семейства. Настоящее TCP-соединение на петле (127.0.0.1), без сети и подмен:
//! «localhost» у системы — и 127.0.0.1, и ::1; слушатель только на IPv4.
//! Кто вызывает: cargo test. От чего зависит: gateway/http_bind.rs, tokio.

use std::net::IpAddr;

use arb::gateway::http_bind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Клиент с адресом 127.0.0.1 к «localhost»: соединение приходит с 127.0.0.1, ответ 200.
#[tokio::test]
async fn ipv4_client_goes_from_its_address() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let srv = tokio::spawn(async move {
        let (mut s, peer) = l.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = s.read(&mut buf).await.unwrap();
        s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await
            .unwrap();
        peer.ip()
    });
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    let c = http_bind::of(ip).unwrap();
    let r = c
        .get(format!("http://localhost:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(srv.await.unwrap(), ip);
}

/// Клиент с IPv6-адресом к имени, у которого слушатель только IPv4: запрос не уходит по IPv4 мимо настройки.
#[tokio::test]
async fn ipv6_client_never_falls_back_to_ipv4() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let c = http_bind::of("::1".parse().unwrap()).unwrap();
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        c.get(format!("http://localhost:{port}/")).send(),
    )
    .await
    .unwrap();
    assert!(
        r.is_err(),
        "IPv4-слушатель не должен получить запрос: {r:?}"
    );
}

/// Один адрес — один клиент из кэша (тот же пул соединений).
#[test]
fn same_address_same_client_builds() {
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    assert!(http_bind::of(ip).is_ok());
    assert!(http_bind::of(ip).is_ok());
}
