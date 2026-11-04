//! Файл: venues/bitget/account.rs
//! Что делает: балансы спот-счёта Bitget — все и одной монеты (решение 113), и кнопка «Проверить подключение»: ключ
//! принят, права (спот-торговля и вывод; без фьючерсов, маржи и копитрейдинга — урок 19, 29), привязка к IP (вывод —
//! только с IP из белого списка), режим счёта Classic или UTA — явно в тексте (адаптер на Classic API v2), скидка BGB.
//! Кто вызывает: mod.rs (функции контракта), all_balances.rs (Спот).
//! От чего зависит: rest.rs, num.rs, fees.rs (deduct_info), ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip, venue.deposit_to_spot, venue.withdraw_from_trade [bitget].
//!
//! Официально (https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account):
//! - Get Account Assets GET /api/v2/spot/account/assets?coin=&assetType= — available, frozen («Usually frozen when the
//!   limit order is placed»), locked («required to become a fiat merchants»), limitAvailable, uTime. Имя монеты в примере
//!   ответа — строчными («usdt»): сравнение без учёта регистра, наружу — заглавными (проверить на живом ответе).
//! - Get Account Information GET /api/v2/spot/account/info («1 times/1s (UID)») — userId, ips «IP whitelist»,
//!   authorities — коды прав: stow «spot trade» (чтение и запись), wwow «withdrawal», wtow «wallet transfer», coow / cpow
//!   — фьючерсы (ордера / позиции), smow — маржа, ttow — копитрейдинг (запись). Имена — только из этого перечня.
//! - Get Upgrade Status GET /api/v2/spot/account/upgrade-status — status «process» / «success» / «fail», reason.
//!   Прямого «текущий режим счёта» в Classic нет: success — счёт переведён в UTA (отказ), process — переводится (отказ),
//!   иное — классический (значение показывается; проверить на живом ответе).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{fees, num, rest};

/// Права, без которых бот не работает: спот-торговля и вывод.
const REQUIRED: [(&str, &str); 2] = [("stow", "спот-торговля"), ("wwow", "вывод")];
/// Право перевода между счетами (нужно при venue.deposit_to_spot / venue.withdraw_from_trade).
const TRANSFER: (&str, &str) = ("wtow", "перевод между счетами");
/// Права, которых у ключа бота быть не должно (только спот — CLAUDE.md).
const FORBIDDEN: [(&str, &str); 4] = [
    ("coow", "фьючерсы: ордера"),
    ("cpow", "фьючерсы: позиции"),
    ("smow", "маржа"),
    ("ttow", "копитрейдинг"),
];

/// Балансы спот-счёта; coin — одна монета (None — все). Ошибки: ключи, связь, ответ Bitget.
pub async fn balances(ctx: &Ctx, coin: Option<&str>) -> GatewayResult<Vec<Balance>> {
    let q = coin
        .map(|c| vec![("coin", c.to_string())])
        .unwrap_or_default();
    let path = "/api/v2/spot/account/assets";
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    spot_rows(ctx, &data)
}

/// Строки спот-счёта из data Get Account Assets: free = available, locked = frozen + locked.
/// Ошибки: BadResponse — data не массив, нет полей.
pub fn spot_rows(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("assets: data не массив", &data.to_string()))?;
    list.iter()
        .map(|b| {
            let zero = |f: &str| num::dec_opt(b, f).map(|v| v.unwrap_or_default());
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "coin")?.to_ascii_uppercase(),
                free: num::dec(b, "available")?,
                locked: zero("frozen")? + zero("locked")?,
            })
        })
        .collect()
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
    let get = |path: &'static str| async move {
        rest::signed(ctx, Method::GET, path, vec![], None, Op::Other)
            .await
            .map_err(|e| e.message)
    };
    let info = get("/api/v2/spot/account/info").await?;
    let perms: Vec<String> = info
        .get("authorities")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let has = |code: &str| perms.iter().any(|p| p == code);
    let mut need: Vec<(&str, &str)> = REQUIRED.to_vec();
    let moves = ctx.flag("venue.deposit_to_spot").unwrap_or(false)
        || ctx.flag("venue.withdraw_from_trade").unwrap_or(false);
    if moves {
        need.push(TRANSFER);
    }
    let missing: Vec<String> = need
        .iter()
        .filter(|(c, _)| !has(c))
        .map(|(c, n)| format!("{c} ({n})"))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "У ключа Bitget нет прав: {} (есть: {})",
            missing.join(", "),
            perms.join(", ")
        ));
    }
    let extra: Vec<String> = FORBIDDEN
        .iter()
        .filter(|(c, _)| has(c))
        .map(|(c, n)| format!("{c} ({n})"))
        .collect();
    if !extra.is_empty() {
        return Err(format!(
            "У ключа Bitget лишние права: {} — бот только спот, снимите их",
            extra.join(", ")
        ));
    }
    let ip_note = ip_note(ctx, &num::text_opt(&info, "ips").unwrap_or_default())?;
    let mode = mode(&get("/api/v2/spot/account/upgrade-status").await?)?;
    let bgb = match fees::deduct_info(ctx).await {
        Ok(true) => "скидка BGB включена — расчёт по обычной ставке (худший случай)".to_string(),
        Ok(false) => "скидка BGB выключена".to_string(),
        Err(e) => format!("скидка BGB не прочитана: {}", e.message),
    };
    Ok(format!(
        "Подключено. Тип счёта: {mode}. Права есть ({}), {ip_note}; {bgb}. Вывод — только на адреса из адресной книги \
         кабинета Bitget (код 11000)",
        perms.join(", ")
    ))
}

/// Привязка к IP: включена — venue.api_ip должен быть в ips ключа; вывод Bitget — только с белого списка IP.
/// Ошибка — текст причины.
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
        if ips.is_empty() {
            return Err(
                "Ключ Bitget без белого списка IP — вывод Bitget разрешает только с привязанного IP («withdraw coins \
                 through a whitelisted IP address»): привяжите IP сервера к ключу и включите venue.api_ip_bound"
                    .into(),
            );
        }
        return Ok(format!("привязка к IP не сверяется (на Bitget: {shown})"));
    }
    let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
    if !ips.contains(&ip.as_str()) {
        return Err(format!(
            "Привязка к IP включена, но IP {ip} нет в белом списке ключа Bitget (там: {shown})"
        ));
    }
    Ok(format!("ключ привязан к IP {ip}"))
}

/// Режим счёта по upgrade-status. Возвращает: текст «Classic …». Ошибка — счёт UTA или переводится.
fn mode(st: &Value) -> Result<String, String> {
    let status = num::text_opt(st, "status").unwrap_or_default();
    let reason = num::text_opt(st, "reason").unwrap_or_default();
    match status.as_str() {
        "success" => Err(
            "Тип счёта Bitget: UTA (upgrade-status = success) — адаптер работает на Classic API v2, нужен классический \
             счёт"
                .into(),
        ),
        "process" => Err("Тип счёта Bitget: идёт переход в UTA (upgrade-status = process) — дождитесь итога".into()),
        _ => Ok(format!(
            "Classic (upgrade-status = «{status}»{})",
            if reason.is_empty() {
                String::new()
            } else {
                format!(", reason = {reason}")
            }
        )),
    }
}
