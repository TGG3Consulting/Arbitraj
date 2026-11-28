//! Файл: venues/kucoin/settle_withdraw.rs
//! Что делает: расчётный круг Base (вывод USDC) — метка попытки вывода до отправки и сверка после потери ответа.
//! У Withdraw (V3) нет поля клиентского/идемпотентного ID; есть поле remark, которое KuCoin возвращает в истории
//! выводов. Метка попытки (remark, transfers.rs) сохраняется через gateway/settlement_intent.rs ДО POST; после потери
//! ответа вывод ищется в истории выводов монеты по этой метке и сети (только GET, повторной отправки нет).
//! Вне расчётного круга (settlement_intent::active() = false) ничего не меняется.
//! Кто вызывает: transfers.rs (marker, before_send), mod.rs (Venue::resolve_withdraw).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, gateway/pending.rs, gateway/settlement_intent.rs.
//! Настройки: нет своих.
//!
//! Официально (спецификация SDK spec/rest/api/openapi-account-withdrawal.json):
//! - Withdraw (V3) POST /api/v3/withdrawals https://www.kucoin.com/docs-new/api-3470146 — поля тела: currency, chain,
//!   amount, memo, isInner, remark («Remark»), feeDeductType, toAddress, withdrawType; клиентского ID нет.
//! - Get Withdrawal History GET /api/v1/withdrawals (Management, вес 20, право General)
//!   https://www.kucoin.com/docs-new/api-3470145 — currency*, pageSize («Minimum is 10, maximum is 500»); «Items are
//!   paginated and sorted to show the latest first»; items[]: id («Unique ID»), currency, chain («The chain id of
//!   currency»), status, address (в примере доки — замаскирован, поэтому не сравнивается), amount, fee, walletTxId,
//!   remark («Remark»).

use reqwest::Method;

use crate::core::types::{Transfer, WithdrawRequest};
use crate::gateway::errors::{GatewayError, GatewayResult};
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::reply::Pool;
use super::{num, rest};

/// Наибольший pageSize истории выводов — официально «maximum is 500» (Get Withdrawal History).
const PAGE_SIZE: &str = "500";

/// Что делает: «чем найти» попытку вывода — метка remark и время отправки (Unix, секунды).
/// Принимает: метку remark. Возвращает: PendingRef. Ошибок нет.
pub fn marker(remark: &str) -> PendingRef {
    PendingRef {
        sent_at_s: Some(chrono::Utc::now().timestamp()),
        ..PendingRef::withdraw(remark)
    }
}

/// Что делает: в расчётном круге — сохранить метку попытки до POST (settlement_intent::before_send); вне круга —
/// ничего. Принимает: «чем найти». Возвращает: (). Ошибки: как у before_send (шаг уже зарегистрирован, сверка
/// маршрута не прошла, сбой журнала) — тогда POST не отправляется.
pub async fn before_send(p: &PendingRef) -> GatewayResult<()> {
    if crate::gateway::settlement_intent::active() {
        crate::gateway::settlement_intent::before_send(p).await?;
    }
    Ok(())
}

/// Что делает: в расчётном круге — приложить «чем найти» к ошибке «итог неизвестен» (решение 231); вне круга —
/// ошибка как была. Принимает: ошибку, «чем найти». Возвращает: ошибку.
pub fn tag(e: GatewayError, p: &PendingRef) -> GatewayError {
    if crate::gateway::settlement_intent::active() {
        e.pending(p.clone())
    } else {
        e
    }
}

/// Что делает: создан ли вывод, чей ответ потерян, — поиск в истории выводов монеты по своей метке remark и сети
/// запроса. Только GET. Принимает: окружение, исходный запрос, «чем найти». Возвращает: Some(перевод Pending с
/// withdrawalId) — найден (дальше обычный опрос статуса); None — в последних PAGE_SIZE записях монеты не найден.
/// Ошибки: нет своей метки (BadResponse); связь, коды KuCoin, разбор ответа — ядро спросит ещё.
pub async fn resolve(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(remark) = p.client_id.as_deref().filter(|r| !r.is_empty()) else {
        return Err(errors::bad(
            "нет своей метки вывода (remark)",
            &format!("{p:?}"),
        ));
    };
    let q = vec![
        ("currency", req.asset.ticker.clone()),
        ("pageSize", PAGE_SIZE.to_string()),
    ];
    let data = rest::signed(
        ctx,
        Method::GET,
        "/api/v1/withdrawals",
        q,
        None,
        Op::Other,
        Pool::Management,
    )
    .await?;
    let items = num::arr(&data, "items")?;
    let id = pick(items, remark, &req.asset.ticker, &req.network)?;
    Ok(id.map(|id| pending::created(ctx.venue(), req, id)))
}

/// Что делает: withdrawalId записи истории с той же меткой remark, монетой и сетью (chain — chainId, без учёта
/// регистра). Принимает: items[] ответа, метку, тикер, chainId. Возвращает: Some(id); None — нет такой записи.
/// Ошибки: BadResponse — запись с меткой есть, но без id, или таких записей несколько (неоднозначно).
pub fn pick(
    items: &[serde_json::Value],
    remark: &str,
    ticker: &str,
    chain: &str,
) -> GatewayResult<Option<String>> {
    let mut found = items.iter().filter(|r| {
        num::text_opt(r, "remark").as_deref() == Some(remark)
            && num::text_opt(r, "currency").as_deref() == Some(ticker)
            && num::text_opt(r, "chain").is_some_and(|c| c.eq_ignore_ascii_case(chain))
    });
    let Some(rec) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(errors::bad(
            &format!("в истории выводов несколько записей с меткой {remark}"),
            "",
        ));
    }
    num::text_opt(rec, "id")
        .map(Some)
        .ok_or_else(|| errors::bad("запись вывода без id", &rec.to_string()))
}
