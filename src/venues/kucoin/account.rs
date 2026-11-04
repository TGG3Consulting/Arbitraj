//! Файл: venues/kucoin/account.rs
//! Что делает: балансы торгового счёта KuCoin (trade) — все и одной монеты (решение 113), и кнопка «Проверить
//! подключение»: ключ принят, версия ключа совпадает с настройкой, права (без маржи и фьючерсов — урок 19), привязка
//! к IP, высокочастотный ли спот-счёт (ордера адаптера — HF-эндпоинты /api/v1/hf/*) и тип счёта — Classic или UTA,
//! явно в тексте (решение 137, Тигран 04.11): адаптер на Classic API, счёт UTA — отказ проверки.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.api_key_version, venue.api_ip_bound, venue.api_ip, venue.deposit_to_spot, venue.withdraw_from_trade
//! [kucoin].
//!
//! Официально:
//! - Get Account List - Spot GET /api/v1/accounts?currency=&type= (Management, вес 5)
//!   https://www.kucoin.com/docs-new/api-3470125 — «Please deposit funds into the main account first, then use the
//!   Transfer function to move them to the trade account before trading»; type main / trade; balance, available, holds.
//! - Get Apikey Info GET /api/v1/user/api-key (Management, вес 20) https://www.kucoin.com/docs-new/api-3470130 —
//!   apiVersion, permission («General, Spot, Margin, Futures, InnerTransfer, Transfer, Earn»), ipWhitelist, isMaster.
//!   Имена прав в ответе — значения поля permission, а не имена x-api-permission эндпоинтов («Withdrawal», «FlexTransfers»).
//!   Официально: спецификация KuCoin (github.com/Kucoin/kucoin-universal-sdk, spec/rest/api/openapi-account-account.json,
//!   Get API Key Info) — «permission … possible values: General, Spot, Margin, Futures, InnerTransfer, Transfer, Earn»;
//!   openapi-account-subaccount.json — «InnerTransfer (Flex Transfer)»; страница API Key Permissions — у Withdrawal:
//!   «the Transfer permission» (вывод). Живой ответ 04.11.2026 — «General, Futures, Unified, Spot, Earn, InnerTransfer,
//!   Transfer, Margin» (дополнение решения 137).
//! - Get Account Type - Spot GET /api/v1/hf/accounts/opened (Spot, вес 30) https://www.kucoin.com/docs-new/api-3470120 —
//!   «True means the current user is a high-frequency spot user».
//! - Get Account Mode (UTA REST V2) GET /api/ua/v2/account/mode (право General, Management, вес 30)
//!   https://www.kucoin.com/docs-new/v2/rest/ua/get-account-mode — «query the list of unified and classic sub-accounts
//!   and current account mode»; data.selfAccountMode — в примерах документации CLASSIC и UNIFIED (перечень значений
//!   формально не описан: другое значение — отказ проверки с этим значением).

use reqwest::Method;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::{num, rest};

/// Счёт, на котором бот торгует (значение type из документации).
const TRADE_TYPE: &str = "trade";
/// Права, без которых бот не работает.
const REQUIRED: [&str; 3] = ["General", "Spot", "Transfer"];
/// Права, которых у ключа бота быть не должно (только спот — CLAUDE.md; единый счёт бот не поддерживает).
const FORBIDDEN: [&str; 3] = ["Margin", "Futures", "Unified"];

/// Балансы торгового счёта; currency — одна монета (None — все). Ошибки: ключи, связь, ответ KuCoin.
pub async fn balances(ctx: &Ctx, currency: Option<&str>) -> GatewayResult<Vec<Balance>> {
    let mut q = vec![("type", TRADE_TYPE.to_string())];
    if let Some(c) = currency {
        q.insert(0, ("currency", c.to_string()));
    }
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v1/accounts",
        q,
        None,
        Op::Other,
        Pool::Management,
    )
    .await?;
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("accounts: data не массив", &data.to_string()))?;
    list.iter()
        .filter(|b| num::text_opt(b, "type").as_deref() == Some(TRADE_TYPE))
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "currency")?,
                free: num::dec(b, "available")?,
                locked: num::dec(b, "holds")?,
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
    let get = |path: &'static str, pool| async move {
        rest::signed(ctx, Method::GET, path, vec![], None, Op::Other, pool)
            .await
            .map_err(|e| e.message)
    };
    let info = get("/api/v1/user/api-key", Pool::Management).await?;
    let want = ctx.text("venue.api_key_version").map_err(|e| e.message)?;
    let got = num::text_opt(&info, "apiVersion").unwrap_or_default();
    if got != want {
        return Err(format!(
            "Версия ключа KuCoin {got}, а venue.api_key_version = {want} — задайте версию ключа со страницы API Management"
        ));
    }
    let perms = split(&num::text_opt(&info, "permission").unwrap_or_default());
    let mut need: Vec<&str> = REQUIRED.to_vec();
    let moves = ctx.flag("venue.deposit_to_spot").unwrap_or(false)
        || ctx.flag("venue.withdraw_from_trade").unwrap_or(false);
    if moves {
        need.push("InnerTransfer");
    }
    let missing: Vec<&str> = need
        .into_iter()
        .filter(|p| !perms.iter().any(|x| x == p))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "У ключа KuCoin нет прав: {} (есть: {}). Transfer — «Вывод средств», InnerTransfer — «Разрешить гибкие переводы»",
            missing.join(", "),
            perms.join(", ")
        ));
    }
    let extra: Vec<&str> = FORBIDDEN
        .into_iter()
        .filter(|p| perms.iter().any(|x| x == p))
        .collect();
    if !extra.is_empty() {
        return Err(format!(
            "У ключа KuCoin лишние права: {} — бот только спот, снимите их",
            extra.join(", ")
        ));
    }
    let ips = split(&num::text_opt(&info, "ipWhitelist").unwrap_or_default());
    let ip_note = if ctx.flag("venue.api_ip_bound").map_err(|e| e.message)? {
        let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
        if !ips.contains(&ip) {
            return Err(format!(
                "Привязка к IP включена, но IP {ip} нет в белом списке ключа KuCoin (там: {})",
                list(&ips)
            ));
        }
        format!("ключ привязан к IP {ip}")
    } else {
        format!("привязка к IP не требуется (на KuCoin: {})", list(&ips))
    };
    let hf = get("/api/v1/hf/accounts/opened", Pool::Spot).await?;
    if hf.as_bool() != Some(true) {
        return Err(format!(
            "Тип спот-счёта KuCoin: не HF (hf/accounts/opened = {hf}) — ордера адаптера идут через /api/v1/hf/*, нужен HF"
        ));
    }
    let mode = get("/api/ua/v2/account/mode", Pool::Management).await?;
    let mode = num::text_opt(&mode, "selfAccountMode").unwrap_or_default();
    match mode.as_str() {
        "CLASSIC" => {}
        "UNIFIED" => {
            return Err(
                "Тип счёта KuCoin: UTA (selfAccountMode = UNIFIED) — адаптер работает на Classic API, нужен классический счёт"
                    .into(),
            );
        }
        other => {
            return Err(format!(
                "Тип счёта KuCoin не распознан: selfAccountMode = «{other}»"
            ));
        }
    }
    Ok(format!(
        "Подключено. Тип счёта: Classic (selfAccountMode = CLASSIC), спот — HF (высокочастотный, hf/accounts/opened = true). \
         Версия ключа {got}, права есть ({}), {ip_note}",
        perms.join(", ")
    ))
}

/// Строка списка → элементы (разделители: запятая, точка с запятой, пробелы). Ошибок нет.
fn split(s: &str) -> Vec<String> {
    s.split([',', ';', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// Список IP для текста.
fn list(ips: &[String]) -> String {
    if ips.is_empty() {
        "без привязки".into()
    } else {
        ips.join(", ")
    }
}
