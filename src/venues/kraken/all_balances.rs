//! Файл: venues/kraken/all_balances.rs
//! Что делает: балансы ВСЕХ счетов Kraken для панели «Балансы» (решение 144) — только чтение, одним запросом BalanceEx:
//! «Спот» (активы без суффикса — на них торгует бот) и «Earn и прочие кошельки» (активы с суффиксом .B, .F, .T, .S, .M —
//! на споте не торгуются, всё в «занято»). Пустой ответ — пустой счёт, не ошибка (урок 27). Фьючерсы Kraken —
//! отдельный API со своими ключами (https://futures.kraken.com) — бот только спот и их не читает (сказано в сведениях).
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» ≈ раз в 3 с, пока открыта.
//! От чего зависит: account.rs (BalanceEx), ctx.rs.
//! Настройки: нет своих.
//!
//! Официально (OpenAPI spot-rest.yaml, BalanceEx): суффиксы «.B» (yield-bearing), «.F» (устарел 13.11.2025), «.T»
//! (tokenized), «.S», «.M» — только чтение. Один запрос раз в 3 с = +1 к счётчику REST ключа каждые 3 с (убывание у
//! Starter — 0,33 в с): панель не держать открытой во время торговли на счёте уровня Starter (README адаптера).

use crate::core::types::{AccountBalances, Balance};
use crate::gateway::errors::GatewayResult;

use super::account;
use super::ctx::Ctx;

/// Что делает: читает счета. Возвращает: «Спот», «Earn и прочие кошельки», «Фьючерсы» (только сведения).
/// Ошибки: как BalanceEx (ни один счёт не прочитан).
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let (spot, other) = account::read(ctx).await?;
    Ok(vec![
        account_of("Спот", spot, None),
        account_of(
            "Earn и прочие кошельки (.B .F .T .S .M)",
            other,
            Some("на споте не торгуются — показано в «занято»".into()),
        ),
        AccountBalances {
            account: "Фьючерсы".into(),
            balances: vec![],
            error: None,
            note: Some(
                "отдельный API Kraken Futures со своими ключами — бот только спот, не читает"
                    .into(),
            ),
        },
    ])
}

/// Счёт без ошибки (нулевые строки отбрасываются). Ошибок нет.
fn account_of(name: &str, rows: Vec<Balance>, note: Option<String>) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: rows
            .into_iter()
            .filter(|b| !b.free.is_zero() || !b.locked.is_zero())
            .collect(),
        error: None,
        note,
    }
}
