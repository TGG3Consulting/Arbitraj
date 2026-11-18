//! Файл: venues/xt/deposit_addr.rs
//! Что делает: адрес депозита XT.com по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса
//! (урок 25, решение 118 Я-5: адрес нужен в предпроверке каждой сделки — повторных запросов нет). Адрес с непустым tag
//! (memo) — ошибка «требует memo — не поддерживается» (вывод на такой адрес без memo не зачислится); не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально: GET /v4/deposit/address (https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetDepositAddress,
//! `_deposit_withdrawal/depositAddressGet.md`) — chain и currency обязательны (значения — из support/currency); ответ
//! result {address, memo}. Коды — errorCode.md (DEPOSIT_001…008).
//! Решение 379 (задача t165, 17.11.2026): метода создания адреса в документации нет; коды errorCode.md DEPOSIT_006 «No
//! deposit address, please try it later» и DEPOSIT_007 «Address is being generated, please try it later» — адрес
//! создаёт сама XT (повтор позже), прямо «GET создаёт адрес» документация не пишет. Поведение прежнее (вопрос Тиграну —
//! docs/TASKS.md, t165).

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::addr_book::AddrBook;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::{Ctx, currency};
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса депозита: (монета, сеть) → адрес. Дешёво клонируется.
/// Решение 411: ключ памяти — ещё и отпечаток ключа API биржи (смена ключа — старый адрес не берётся).
#[derive(Clone, Default)]
pub struct DepositAddrs {
    book: AddrBook<String>,
}

/// Что делает: адрес депозита монеты в сети — из памяти или запросом.
/// Принимает: окружение, кэш, монету, код сети. Возвращает: адрес.
/// Ошибки: InvalidAddress — адреса нет, он ещё создаётся или требует memo; связь, коды XT.
pub async fn deposit_address(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    // Решение 411: память — под отпечатком ключа API биржи и по режиму задачи (gateway/addr_book.rs).
    let mark = ctx.vault.mark(super::ctx::SECRET_API_KEY);
    if let Some(a) = addrs.book.lookup(mark, &asset.ticker, network)? {
        return Ok(a);
    }
    let p = vec![
        ("chain", network.to_string()),
        ("currency", currency(&asset.ticker)),
    ];
    let data = match rest::signed(ctx, Method::GET, "/v4/deposit/address", p, None, Op::Other).await
    {
        Ok(d) => d,
        Err(e) if e.message.contains("DEPOSIT_006") || e.message.contains("DEPOSIT_007") => {
            let why = format!(
                "Адреса депозита XT {} в сети {network} пока нет ({}) — XT его создаёт, повторите позже",
                asset.ticker, e.message
            );
            return Err(invalid(ctx, why));
        }
        Err(e) => return Err(e),
    };
    if let Some(memo) = num::text_opt(&data, "memo").filter(|t| !t.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита XT {} в сети {network} требует memo ({memo}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(data.to_string()));
    }
    let address = num::text_opt(&data, "address").ok_or_else(|| {
        invalid(
            ctx,
            format!(
                "У {} нет адреса депозита XT в сети {network} — создайте его в кабинете (Активы → Депозит)",
                asset.ticker
            ),
        )
        .with_raw(data.to_string())
    })?;
    addrs.book.keep(
        mark,
        asset.ticker.clone(),
        network.to_string(),
        address.clone(),
    );
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой XT. Ошибок нет.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
