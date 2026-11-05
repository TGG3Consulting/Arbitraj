//! Файл: gateway/http_bind.rs
//! Что делает: решение 178 (Тигран, 05.11.2026) — с какого IP-адреса сервера бот ходит по REST к площадке
//! (venue.local_ip). Живой случай 05.11: у ключа XT в кабинете можно привязать только IPv4 (37.59.21.151), а сервер
//! по умолчанию идёт к sapi.xt.com (CloudFront, есть адреса IPv6) по IPv6 — XT отвечает AUTH_104 «Unbound IP request».
//! Не задано — общий клиент, как раньше (система сама выбирает адрес). Задано — клиент с этим исходящим адресом
//! (ClientBuilder::local_address) и DNS только того же семейства: hyper привязывает исходящий адрес лишь к адресу
//! назначения своего семейства (hyper-util connect/http.rs bind_local_address), поэтому без отбора IPv6 запрос
//! ушёл бы по IPv6 мимо настройки. Нет у площадки адреса нужного семейства — честная ошибка «нет адреса IPv4».
//! Меняется на лету: настройка читается на каждый запрос; клиенты кэшируются по адресу (пул соединений сохраняется).
//! WebSocket настройка не затрагивает (подключения WebSocket идут не через reqwest).
//! Кто вызывает: main.rs (builder — общий клиент), venues/*/ctx.rs (client) — у бирж; DEX (свой узел) не использует.
//! От чего зависит: reqwest (local_address, dns_resolver — https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html),
//! tokio (lookup_host), settings/store.rs.
//! Настройки: venue.local_ip [площадка] (необязательная).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};

use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::store::Store;

/// Имя настройки исходящего адреса площадки.
pub const SETTING: &str = "venue.local_ip";

/// Общие свойства всех HTTP-клиентов бота: редиректы выключены (в адресе запроса может быть токен, его нельзя
/// отдавать другому адресу). Одно место — у общего клиента и у клиентов с исходящим адресом. Ошибок нет.
pub fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
}

/// DNS только одного семейства адресов (IPv4 или IPv6) — того же, что исходящий адрес.
struct Family {
    v4: bool,
}

impl reqwest::dns::Resolve for Family {
    /// Системный DNS (tokio lookup_host) с отбором по семейству. Ошибки: DNS, нет адреса нужного семейства.
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let v4 = self.v4;
        let host = name.as_str().to_string();
        Box::pin(async move {
            let all = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let addrs: Vec<SocketAddr> = all.filter(|a| a.is_ipv4() == v4).collect();
            if addrs.is_empty() {
                let fam = if v4 { "IPv4" } else { "IPv6" };
                return Err(format!("у {host} нет адреса {fam} ({SETTING})").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Клиенты с исходящим адресом — по адресу (одинаковый адрес у разных площадок — один клиент и пул соединений).
fn cache() -> &'static Mutex<HashMap<IpAddr, reqwest::Client>> {
    static C: OnceLock<Mutex<HashMap<IpAddr, reqwest::Client>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// HTTP-клиент площадки. Принимает: общий клиент, настройки, id площадки.
/// Возвращает: общий клиент (venue.local_ip не задан) или клиент с этим исходящим адресом.
/// Ошибки: NotConfigured — значение не IP-адрес; Internal — клиент не собрался.
pub fn client(
    base: &reqwest::Client,
    settings: &Store,
    venue: &str,
) -> Result<reqwest::Client, GatewayError> {
    let Some(v) = settings.resolve(SETTING, Some(venue), None) else {
        return Ok(base.clone());
    };
    let text = v.as_str().unwrap_or_default().trim().to_string();
    let ip: IpAddr = text.parse().map_err(|_| {
        GatewayError::new(
            ErrorCode::NotConfigured,
            None,
            format!("{SETTING} «{text}» — не IP-адрес"),
        )
    })?;
    of(ip)
}

/// Клиент с исходящим адресом ip (из кэша или новый). Ошибки: Internal — клиент не собрался.
pub fn of(ip: IpAddr) -> Result<reqwest::Client, GatewayError> {
    let mut c = cache().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(cl) = c.get(&ip) {
        return Ok(cl.clone());
    }
    let cl = builder()
        .local_address(ip)
        .dns_resolver(Family { v4: ip.is_ipv4() })
        .build()
        .map_err(|e| {
            GatewayError::new(
                ErrorCode::Internal,
                None,
                format!("HTTP-клиент с адреса {ip} ({SETTING}) не собран: {e}"),
            )
        })?;
    c.insert(ip, cl.clone());
    Ok(cl)
}
