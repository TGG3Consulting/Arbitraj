//! Файл: venues/bybit/deposit_addr.rs
//! Что делает: адрес депозита Bybit по монете и сети. Кэш в памяти по (монета, сеть) на время работы процесса (как
//! решение 118, Я-5 у MEXC: адрес нужен в предпроверке каждой сделки). Адрес с непустым tag — ошибка «требует tag — не
//! поддерживается» (урок 10, Я-15: контракт отдаёт только адрес — вывод без tag не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официальная документация «Get Master Deposit Address» GET /v5/asset/deposit/query-address?coin=&chainType=
//! https://bybit-exchange.github.io/docs/v5/asset/deposit/master-deposit-addr — «Query the deposit address information
//! of MASTER account»; chainType — «Please use the value of >> chain from coin-info»; ответ: coin, chains[]: chain,
//! chainType, addressDeposit, tagDeposit. Лимит «300 req/min». Депозит зачисляется на FUND («Your funds will be
//! deposited into FUND wallet by default», Set Deposit Account) — перевод на торговый счёт: fund_move.rs.
//! Решение 379 (задача t165, 17.11.2026): метода создания адреса депозита основного аккаунта в документации нет (папка
//! v5/asset/deposit репозитория документации: master-deposit-addr, sub-deposit-addr, set-deposit-acct, записи); создаёт
//! ли GET адрес, документация не говорит. Поведение прежнее: нет адреса — отказ (вопрос Тиграну — docs/TASKS.md, t165).

use crate::core::types::Asset;
use crate::gateway::addr_book::{AddrBook, Mark};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса депозита: (монета, сеть) → адрес. Дешёво клонируется.
/// Решение 411: ключ памяти — ещё и отпечаток ключа API биржи (смена ключа — старый адрес не берётся).
#[derive(Clone, Default)]
pub struct DepositAddrs {
    book: AddrBook<String>,
}

/// Что делает: адрес депозита монеты в сети (chain) — из памяти или запросом.
/// Принимает: окружение, кэш, монету, сеть. Возвращает: адрес.
/// Ошибки: InvalidAddress — адреса в сети нет или он требует tag; связь, коды Bybit.
pub async fn deposit_address(
    ctx: &Ctx,
    addrs: &DepositAddrs,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    // Решение 411: память — под отпечатком ключа API биржи и по режиму задачи (gateway/addr_book.rs).
    let mark = Mark::read(&ctx.vault, super::ctx::SECRET_API_KEY);
    if let Some(a) = addrs.book.lookup(&mark, &asset.ticker, network)? {
        return Ok(a);
    }
    let p = vec![
        ("coin", asset.ticker.clone()),
        ("chainType", network.to_string()),
    ];
    let data = rest::get(ctx, "/v5/asset/deposit/query-address", p, Op::Other).await?;
    let found = data.get("chains").and_then(|c| c.as_array()).and_then(|a| {
        a.iter()
            .find(|r| num::text_opt(r, "chain").as_deref() == Some(network))
    });
    let found = found.ok_or_else(|| {
        invalid(
            ctx,
            format!(
                "У {} нет адреса депозита Bybit в сети {network} — создайте его в кабинете Bybit (Deposit)",
                asset.ticker
            ),
        )
        .with_raw(data.to_string())
    })?;
    if let Some(tag) = num::text_opt(found, "tagDeposit").filter(|t| !t.trim().is_empty()) {
        return Err(invalid(
            ctx,
            format!(
                "Адрес депозита Bybit {} в сети {network} требует tag ({tag}) — не поддерживается",
                asset.ticker
            ),
        )
        .with_raw(found.to_string()));
    }
    let address = num::text(found, "addressDeposit")?;
    addrs.book.keep(
        &mark,
        asset.ticker.clone(),
        network.to_string(),
        address.clone(),
    );
    Ok(address)
}

/// Ошибка InvalidAddress с площадкой Bybit.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
