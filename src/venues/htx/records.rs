//! Файл: venues/htx/records.rs
//! Что делает: общее у истории депозитов и выводов HTX — запрос последних записей (одна страница наибольшего размера,
//! новые первыми) и перевод записи в формат ядра (Transfer).
//! Кто вызывает: deposits.rs, transfers.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, core/types.
//! Настройки: нет.
//!
//! Официально — Search for Existed Withdraws and Deposits GET /v1/query/deposit-withdraw («20times/2s», право Read)
//! https://huobiapi.github.io/docs/spot/v1/en/#search-for-existed-withdraws-and-deposits: type* deposit / withdraw,
//! currency (пусто — все монеты), from («the transfer id to begin search»), size «[1-500]» (по умолчанию 100), direct
//! «prev» (по возрастанию) / «next» (по убыванию; по умолчанию prev). Поиска по хешу нет. Поля: id, type, currency,
//! tx-hash, chain, amount, address, address-tag, fee, state, error-code / error-msg (провал вывода), created-at,
//! updated-at. С какой записи начинает direct=next без from, документация не говорит (ожидается — с последней):
//! проверить на живом ответе.

use chrono::Utc;
use serde_json::Value;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus};
use crate::gateway::errors::GatewayResult;

use super::ctx::{Ctx, currency, ticker};
use super::{num, rest};

/// Наибольший размер страницы — официально «[1-500]».
pub const SIZE_MAX: &str = "500";

/// Что делает: последние записи истории. Принимает: вид (deposit / withdraw), тикер (None — все монеты).
/// Возвращает: массив записей. Ошибки: ключи, связь, коды HTX, data не массив.
pub async fn fetch(ctx: &Ctx, kind: &str, coin: Option<&str>) -> GatewayResult<Vec<Value>> {
    let mut q = vec![
        ("type", kind.to_string()),
        ("size", SIZE_MAX.to_string()),
        ("direct", "next".to_string()),
    ];
    if let Some(t) = coin {
        q.push(("currency", currency(t)));
    }
    let data = rest::get(ctx, "/v1/query/deposit-withdraw", q).await?;
    match data {
        Value::Array(a) => Ok(a),
        Value::Null => Ok(vec![]),
        other => Err(super::errors::bad(
            "deposit-withdraw: data не массив",
            &other.to_string(),
        )),
    }
}

/// Перевод формата ядра из записи (вывод или депозит). Ошибки: BadResponse — нет currency или amount.
pub fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let t = ticker(&num::text(rec, "currency")?);
    let fee = num::dec_opt(rec, "fee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f.abs(), t.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: t,
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chain").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "tx-hash"),
        fee,
        updated_at: Utc::now(),
    })
}
