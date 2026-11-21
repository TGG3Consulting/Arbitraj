//! Файл: venues/htx/all_balances.rs
//! Что делает: балансы ВСЕХ счетов HTX для панели «Балансы» (решение 144, урок 24) — только чтение: каждый счёт из
//! списка счетов пользователя (спот, маржа, OTC, баллы, инвестиции, займы, сетка, earn…) своими строками; запросы —
//! параллельно; сбой одного счёта — в его error, остальные показываются. Фьючерсы — отдельный API деривативов HTX (в
//! спот-документации баланса фьючерсов нет) — не читаются, о чём сказано в note спота.
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» раз в ui.balances_refresh_ms, пока открыта.
//! От чего зависит: accounts.rs (список), account.rs (строки счёта), ctx.rs.
//! Настройки: как у rest.rs.
//!
//! Официально: Get all Accounts of the Current User GET /v1/account/accounts и Get Account Balance of a Specific Account
//! GET /v1/account/accounts/{account-id}/balance («100 times/2s» каждый) https://huobiapi.github.io/docs/spot/v1/en/.
//! Нет ни одного счёта (новый UID) — «счёт не открыт» (урок 27): пустой список, не ошибка.

use futures_util::future::join_all;

use crate::core::types::{AccountBalances, Balance};
use crate::gateway::errors::GatewayResult;

use super::account;
use super::accounts::{self, Account};
use super::ctx::Ctx;

/// Название счёта для панели по type документации (незнакомый type — как пришёл). Ошибок нет.
pub fn name(a: &Account) -> String {
    let base = match a.kind.as_str() {
        "spot" => "Спот",
        "margin" => "Маржа изолированная",
        "super-margin" => "Маржа кросс",
        "otc" => "OTC",
        "point" => "Баллы HB (point)",
        "investment" => "Инвестиции",
        "borrow" => "Займы",
        "grid-trading" => "Сеточный бот",
        "deposit-earning" => "Earn (депозит)",
        "otc-options" => "OTC-опционы",
        other => other,
    };
    if a.subtype.is_empty() {
        base.to_string()
    } else {
        format!("{base} ({})", a.subtype)
    }
}

/// Что делает: читает все счета параллельно. Возвращает: счета по порядку списка HTX.
/// Ошибки: список счетов не получен (ключ, связь); ошибка отдельного счёта — в его error.
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let list = accounts::list(ctx).await?;
    let reads = list.iter().map(|a| account::of_account(ctx, a.id));
    let got = join_all(reads).await;
    Ok(list
        .iter()
        .zip(got)
        .map(|(a, r)| match r {
            Ok(rows) => account_of(a, rows),
            Err(e) => AccountBalances {
                account: name(a),
                balances: vec![],
                error: Some(e.message),
                note: None,
            },
        })
        .collect())
}

/// Счёт панели: только ненулевые строки; у спота — пояснение про фьючерсы; счёт не в state working — в note.
fn account_of(a: &Account, rows: Vec<Balance>) -> AccountBalances {
    let mut notes = vec![];
    if a.state != accounts::WORKING && !a.state.is_empty() {
        notes.push(format!("state = {}", a.state));
    }
    if a.kind == accounts::SPOT {
        notes.push("фьючерсы HTX — отдельный API деривативов, здесь не показаны".to_string());
    }
    AccountBalances {
        account: name(a),
        balances: rows
            .into_iter()
            .filter(|b| !b.free.is_zero() || !b.locked.is_zero())
            .collect(),
        error: None,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
    }
}
