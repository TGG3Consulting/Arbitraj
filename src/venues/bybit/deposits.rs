//! Файл: venues/bybit/deposits.rs
//! Что делает: статус депозита Bybit по хешу транзакции; хеши сравниваются через gateway::tx_hash::same.
//! Кто вызывает: mod.rs (deposit_status из контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, gateway/tx_hash.rs.
//! Настройки: нет своих.
//!
//! Официальная документация:
//! - «Get Deposit Records (on-chain)» GET /v5/asset/deposit/query-record https://bybit-exchange.github.io/docs/v5/asset/deposit/deposit-record
//!   — coin, txID («data generated before Jan 1, 2024 cannot be queried using txID»); rows[]: coin, chain, amount, txID,
//!   status, depositFee, depositType («0: normal deposit, 10: … daily deposit limit, 20: hit AML …, 50: … blocked due to
//!   an account or compliance restriction»), id. Лимит «100 req/min».
//! - enum depositStatus https://bybit-exchange.github.io/docs/v5/enum#depositstatus — 0 unknown, 1 toBeConfirmed,
//!   2 processing, 3 success («final status»), 4 deposit failed, 7 Rollback processing, 70011 Deposit rolled back,
//!   70012 «Deposit remains successful after rollback review», 70013 rollback failed — manual review, 10011 «pending to
//!   be credited to funding pool», 10012 «Credited to funding pool successfully».
//!
//! «Зачислено» = 3 или 70012. 10012 — что это для спот-депозита (не «final»), в документации не ясно: ждём 3
//! (вопрос Тиграну). 4 и 70011 — не зачислен; depositType 10/20/50 — заблокирован (нужно действие в кабинете).

use chrono::Utc;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tx_hash;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Статус депозита по хешу транзакции. Ошибки: DepositNotCredited (не виден, не зачислен, заблокирован), формат.
pub async fn deposit_status(ctx: &Ctx, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
    let p = vec![
        ("coin", asset.ticker.clone()),
        ("txID", tx_hash.to_string()),
    ];
    let data = rest::get(ctx, "/v5/asset/deposit/query-record", p, Op::Other).await?;
    let rows = num::arr(&data, "rows")?;
    let rec = rows
        .iter()
        .find(|r| num::text_opt(r, "txID").is_some_and(|t| tx_hash::same(&t, tx_hash)));
    let rec =
        rec.ok_or_else(|| not_credited(ctx, format!("Депозит {tx_hash} пока не виден Bybit")))?;
    let kind = num::text_opt(rec, "depositType").unwrap_or_else(|| "0".into());
    if kind != "0" {
        return Err(not_credited(
            ctx,
            format!(
                "Bybit: депозит {tx_hash} заблокирован, depositType {kind} (10 — дневной предел, 20 — AML, 50 — \
                 ограничение аккаунта) — нужно действие в кабинете Bybit"
            ),
        )
        .with_raw(rec.to_string()));
    }
    let status = match num::int(rec, "status")? {
        3 | 70012 => TransferStatus::Credited,
        0 | 1 | 2 | 7 | 70013 | 10011 | 10012 => TransferStatus::Sent,
        s @ (4 | 70011) => {
            return Err(not_credited(
                ctx,
                format!(
                    "Bybit: депозит {tx_hash} не зачислен, статус {s} (4 failed, 70011 rolled back)"
                ),
            )
            .with_raw(rec.to_string()));
        }
        other => {
            return Err(errors::bad(
                &format!("неизвестный статус депозита {other}"),
                &rec.to_string(),
            ));
        }
    };
    let ticker = num::text(rec, "coin")?;
    let fee = num::dec_opt(rec, "depositFee")?
        .filter(|f| !f.is_zero())
        .map(|f| Cost::new(CostKind::WithdrawFee, f, ticker.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: num::text_opt(rec, "id").unwrap_or_else(|| tx_hash.to_string()),
        tx_hash: num::text_opt(rec, "txID"),
        fee,
        updated_at: Utc::now(),
    })
}

/// Ошибка DepositNotCredited с площадкой Bybit.
fn not_credited(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::DepositNotCredited, None, msg))
}
