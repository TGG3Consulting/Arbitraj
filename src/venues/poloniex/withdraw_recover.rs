//! Файл: venues/poloniex/withdraw_recover.rs
//! Что делает: сверка вывода расчётного круга после потери ответа POST /v2/wallets/withdraw — находит в истории
//! исходный вывод по своему withdrawOrderId и проверяет, что запись описывает тот же запрос (монета, адрес, сумма).
//! Кто вызывает: mod.rs (Venue::resolve_withdraw) — ядро core/settlement_withdrawal.rs.
//! От чего зависит: transfers.rs (find — поиск в /wallets/activity), wallet.rs (подтверждение сетевого имени монеты),
//! num.rs, ctx.rs, gateway/pending.rs.
//! Настройки: через transfers.rs/activity.rs — venue.history_window_s [poloniex]; через wallet.rs —
//! venue.networks_fresh_ms [poloniex].
//!
//! Официально (https://api-docs.poloniex.com/spot/api/private/wallet):
//! - Withdraw Currency V2: withdrawOrderId «User-defined withdrawal ID (up to 32 characters)», «the backend uses this as
//!   the unique key for the withdrawal order», «duplicate values will be treated as idempotent submissions».
//! - Wallets Activity Records, withdrawals: withdrawalRequestsId, currency, address, amount («the total amount withdrawn
//!   including the fee»), fee, timestamp, status, txid, withdrawOrderId, withdrawalType, toAccount. Поля network в
//!   записи вывода НЕТ — сеть запроса сверяется только через уникальный withdrawOrderId.
//! Факты записанных выводов (tests/data/poloniex_withdraw_alias_2026-11-23):
//! - ledger.json: вывод BNB по сети BSC в истории назван currency = BSC — именем монеты в сети (networkList.coin
//!   справочника), а не спотовым тикером; такое имя принимается, только если справочник подтверждает его для этой
//!   монеты и сети (withdraw_alias.rs, Book::confirms).
//! - sndk_2026-11-24.json: запрошено 3.233946146, в истории amount 3.23394614 (0.369472574 → 0.36947257) — биржа
//!   хранит сумму с меньшим числом знаков, отбрасывая лишние; сумма сверяется после такого же отбрасывания.

use rust_decimal::{Decimal, RoundingStrategy};

use crate::core::types::{Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::{num, transfers, wallet};

/// Что делает: исходный вывод по своему withdrawOrderId. Принимает: окружение, справочник сетей, исходный запрос,
/// «чем найти» (client_id — withdrawOrderId, venue_ref — withdrawalRequestsId, если был известен).
/// Возвращает: Some(перевод Pending с withdrawalRequestsId) — запись найдена и совпадает с запросом; None — записи
/// нет или реквизиты не подтверждены (ядро оставляет вывод на сверке). Ошибки: BadResponse — amount записи не число.
pub(super) async fn settlement(
    ctx: &Ctx,
    nets: &wallet::Networks,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(client) = p.client_id.as_deref() else {
        return Ok(None);
    };
    let Some(row) = transfers::find(ctx, "withdrawOrderId", client).await else {
        return Ok(None);
    };
    let Some(reference) = num::text_opt(&row, "withdrawalRequestsId") else {
        return Ok(None);
    };
    let address = req.address.clone();
    let created = crate::gateway::pending::created(ctx.venue(), req, reference.clone());
    let same_coin = num::text_opt(&row, "currency").is_some_and(|c| {
        c.eq_ignore_ascii_case(&created.asset.ticker)
            || nets.confirms(ctx, &created, &c).unwrap_or(false)
    });
    let same_amount =
        num::dec_opt(&row, "amount")?.is_some_and(|a| amount_matches(a, created.amount));
    if p.venue_ref.as_ref().is_some_and(|id| id != &reference)
        || !same_coin
        || !num::text_opt(&row, "address").is_some_and(|v| v.eq_ignore_ascii_case(&address))
        || !same_amount
    {
        return Ok(None);
    }
    Ok(Some(created))
}

/// Что делает: сумма записи истории — та же, что в запросе: запрос, усечённый (без округления вверх) до числа знаков,
/// которое биржа вернула в записи. Принимает: amount записи, сумму запроса. Возвращает: совпадают ли. Ошибок нет.
pub fn amount_matches(recorded: Decimal, requested: Decimal) -> bool {
    recorded == requested.round_dp_with_strategy(recorded.scale(), RoundingStrategy::ToZero)
}
