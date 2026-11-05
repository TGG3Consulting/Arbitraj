//! Файл: venues/phemex/history.rs
//! Что делает: общее у истории депозитов и выводов Phemex — запрос последних записей (limit — настройка) и перевод
//! записи в формат ядра (Transfer): сумма и комиссия «…Ev» — в масштабе valueScale монеты записи.
//! Кто вызывает: deposits.rs, transfers.rs.
//! От чего зависит: rest.rs, num.rs, symbols.rs (масштабы), ctx.rs, core/types.
//! Настройки: venue.history_rows [phemex] — сколько последних записей читать (limit; «default 20», наибольшее значение
//! в документации не указано). Не задана — поиск депозита и вывода не работает, вывод не отправляется (искать его
//! после отправки было бы нечем — урок 7).
//!
//! Официально (Deposit And Withdraw REST API, https://phemex-docs.github.io/): Query deposit history records GET
//! /phemex-deposit/wallets/api/depositHist?currency=&offset=&limit=&withCount= и Query withdraw history records GET
//! /phemex-withdraw/wallets/api/withdrawHist?currency=&chainName=&offset=&limit=&withCount= — поиска по хешу или id нет,
//! ищем в первой странице. Поля: id, amountEv (у вывода «received amount»), feeEv (у вывода «withdraw real fee»),
//! chainName, currency, txHash, status, address, createdAt / submitedAt. Противоречие: в таблице депозита сумма
//! названа amount, в примере — amountEv; берётся amountEv (урок 2, проверить на живом).

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::rest::Params;
use super::symbols::Symbols;
use super::{num, rest};

/// Сколько записей читать: venue.history_rows. Ошибка NotConfigured — не задана.
pub fn rows(ctx: &Ctx) -> GatewayResult<u64> {
    ctx.int("venue.history_rows")
}

/// Что делает: последние записи истории. Принимает: путь (depositHist / withdrawHist), доп. параметры.
/// Возвращает: записи. Ошибки: NotConfigured (limit), ключи, связь, коды Phemex, data не массив.
pub async fn fetch(ctx: &Ctx, path: &str, mut q: Params) -> GatewayResult<Vec<Value>> {
    q.push(("offset", "0".to_string()));
    q.push(("limit", rows(ctx)?.to_string()));
    let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
    num::rows(data, path)
}

/// Перевод формата ядра из записи (вывод или депозит). Принимает: запись, справочник (масштаб монеты), ID площадки,
/// статус. Ошибки: BadResponse — нет currency, amountEv или масштаба монеты.
pub fn transfer(
    ctx: &Ctx,
    symbols: &Symbols,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "currency")?;
    let scale = symbols.scale(ctx, &ticker)?;
    let fee = num::scaled_opt(rec, "feeEv", scale)?
        .map(|f| Cost::new(CostKind::WithdrawFee, f.abs(), ticker.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            ticker: ticker.clone(),
            bsc_contract: None,
        },
        network: num::text_opt(rec, "chainName").unwrap_or_default(),
        amount: num::scaled(rec, "amountEv", scale)?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "txHash"),
        fee,
        updated_at: Utc::now(),
    })
}
