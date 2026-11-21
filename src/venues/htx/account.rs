//! Файл: venues/htx/account.rs
//! Что делает: балансы спот-счёта HTX и кнопка «Проверить подключение»: ключ принят, своя запись ключа (права Read,
//! Trade, Withdraw; статус; привязка к IP — урок 29), спот-счёт открыт и работает, задано ли поле адреса контракта.
//! Кто вызывает: mod.rs (функции контракта), all_balances.rs (строки счёта).
//! От чего зависит: rest.rs, accounts.rs, num.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip, venue.contract_field [htx].
//!
//! Официально (https://huobiapi.github.io/docs/spot/v1/en/):
//! - Get Account Balance of a Specific Account GET /v1/account/accounts/{account-id}/balance («100 times/2s») — data
//!   {id, type, state, list[] {currency, type (trade, frozen, loan, interest, lock, bank), balance, seq-num}}: свободно —
//!   строка trade, занято — frozen и lock (поля available в ответе нет).
//! - Get UID GET /v2/user/uid — data: UID; Query API Key Information GET /v2/user/api-key?uid=&accessKey= — accessKey,
//!   note, permission (readOnly, trade, withdraw — через запятую), ipAddresses, validDays («-1 means permanent»), status
//!   (normal, expired). Право Trade у HTX общее (ордера и переводы спот ↔ фьючерсы): запрет фьючерсов — код бота, не ключ.

use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::accounts::{self, SpotId, WORKING};
use super::ctx::{Ctx, ticker};
use super::{num, rest};

/// Права, без которых бот не работает (значения permission документации).
const REQUIRED: [(&str, &str); 2] = [("trade", "торговля"), ("withdraw", "вывод")];

/// Балансы спот-счёта. Спот-счёта нет — пусто («счёт не открыт», урок 27). Ошибки: ключи, связь, ответ HTX.
pub async fn balances(ctx: &Ctx, spot: &SpotId) -> GatewayResult<Vec<Balance>> {
    match spot.get(ctx).await? {
        Some(id) => of_account(ctx, id).await,
        None => Ok(vec![]),
    }
}

/// Строки одного счёта по id. Ошибки: ключи, связь, ответ HTX.
pub async fn of_account(ctx: &Ctx, id: i64) -> GatewayResult<Vec<Balance>> {
    let path = format!("/v1/account/accounts/{id}/balance");
    let data = rest::get(ctx, &path, vec![]).await?;
    rows(ctx, &data)
}

/// Строки счёта из data: free = trade, locked = frozen + lock; прочие виды строк (loan, interest, bank) не деньги
/// бота и не входят. Ошибки: BadResponse — нет list, поле не число.
pub fn rows(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = num::arr(data, "list")?;
    let mut by: std::collections::BTreeMap<String, Balance> = Default::default();
    for r in list {
        let t = ticker(&num::text(r, "currency")?);
        let amount = num::dec(r, "balance")?;
        let row = by.entry(t.clone()).or_insert_with(|| Balance {
            venue: ctx.venue(),
            asset: t,
            free: Default::default(),
            locked: Default::default(),
        });
        match num::text_opt(r, "type").as_deref() {
            Some("trade") => row.free += amount,
            Some("frozen") | Some("lock") => row.locked += amount,
            _ => {}
        }
    }
    Ok(by.into_values().collect())
}

/// Проверка подключения. Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    match run(ctx).await {
        Ok(details) => ConnectionCheck { ok: true, details },
        Err(details) => ConnectionCheck { ok: false, details },
    }
}

/// Сама проверка. Возвращает: текст «Подключено…». Ошибка — точная причина.
async fn run(ctx: &Ctx) -> Result<String, String> {
    let m = |e: crate::gateway::errors::GatewayError| e.message;
    let uid = rest::get(ctx, "/v2/user/uid", vec![]).await.map_err(m)?;
    let uid = num::to_dec(&uid).ok_or("HTX: /v2/user/uid — в data не число")?;
    let key = ctx.keys().map_err(m)?.key.to_string();
    let q = vec![("uid", uid.to_string()), ("accessKey", key.clone())];
    let data = rest::get(ctx, "/v2/user/api-key", q).await.map_err(m)?;
    let rec = data
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|r| num::text_opt(r, "accessKey").as_deref() == Some(&key))
        })
        .ok_or("HTX: своей записи ключа нет в /v2/user/api-key")?;
    let status = num::text_opt(rec, "status").unwrap_or_default();
    if status != "normal" {
        return Err(format!("Ключ HTX не действует: status = {status}"));
    }
    let perms = num::text_opt(rec, "permission").unwrap_or_default();
    let has = |p: &str| perms.split(',').any(|x| x.trim() == p);
    let missing: Vec<String> = REQUIRED
        .iter()
        .filter(|(p, _)| !has(p))
        .map(|(p, n)| format!("{p} ({n})"))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "У ключа HTX нет прав: {} (есть: {perms})",
            missing.join(", ")
        ));
    }
    let ip = ip_note(ctx, &num::text_opt(rec, "ipAddresses").unwrap_or_default())?;
    let days = num::text_opt(rec, "validDays").unwrap_or_else(|| "не указано".into());
    let all = accounts::list(ctx).await.map_err(m)?;
    let spot = all
        .iter()
        .find(|a| a.kind == accounts::SPOT)
        .ok_or("У пользователя HTX нет спот-счёта (type spot) — счёт не открыт")?;
    if spot.state != WORKING {
        return Err(format!(
            "Спот-счёт HTX {} — state = {}",
            spot.id, spot.state
        ));
    }
    let contract = match ctx.text("venue.contract_field") {
        Ok(f) => format!("адрес контракта — поле {f} справочника сетей"),
        Err(_) => "venue.contract_field не задана: адреса контракта BEP20 нет — монеты HTX не сопоставляются".into(),
    };
    Ok(format!(
        "Подключено. UID {uid}, права {perms}, {ip}, срок ключа (validDays) {days}; спот-счёт {} работает; {contract}. \
         Вывод — только на адреса из списка адресов вывода (добавляются в веб-интерфейсе HTX)",
        spot.id
    ))
}

/// Привязка к IP: включена — venue.api_ip должен быть среди ipAddresses ключа. Ошибка — текст причины.
fn ip_note(ctx: &Ctx, ips_text: &str) -> Result<String, String> {
    let ips: Vec<&str> = ips_text
        .split([',', ';', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .collect();
    let shown = if ips.is_empty() {
        "без привязки".to_string()
    } else {
        ips.join(", ")
    };
    if !ctx.flag("venue.api_ip_bound").map_err(|e| e.message)? {
        return Ok(format!("привязка к IP не сверяется (на HTX: {shown})"));
    }
    let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
    if !ips.contains(&ip.as_str()) {
        return Err(format!(
            "Привязка к IP включена, но IP {ip} нет в списке ключа HTX (там: {shown})"
        ));
    }
    Ok(format!("ключ привязан к IP {ip}"))
}
