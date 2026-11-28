//! Файл: venues/binance/transfers.rs
//! Что делает: вывод с Binance и его статус; поиск вывода по своему withdrawOrderId при неизвестном итоге. Запрос
//! истории выводов очень тяжёлый (вес UID 18000) — между запросами не меньше venue.withdraw_history_min_ms.
//! Кто вызывает: mod.rs (функции контракта), deposits.rs (transfer), settle.rs (find_own — сверка вывода расчётного
//! круга Base).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs, core/types.
//! Настройки: venue.withdraw_wallet [binance] (spot | funding — параметр walletType), venue.withdraw_history_min_ms
//! [binance] (обе проверяются до отправки вывода: после — поздно); решение 253 — venue.withdraw_unready_window_ms
//! [binance]: −4026 / −4024 в пределах окна после своей покупки монеты — WithdrawLocked (повтор ядра, как −4060);
//! дополнение t89 — после своей сделки (покупки ИЛИ продажи) по этой монете, в том числе USDT выручки продажи.
//!
//! Официально (Wallet SAPI):
//! - POST /sapi/v1/capital/withdraw/apply (вес UID 900) — coin*, address*, amount*, withdrawOrderId («client side id
//!   for withdrawal, can be used in withdraw history»), network, walletType («0 spot wallet, 1 funding wallet; default
//!   — current selected wallet»); ответ {"id"}. Travel rule: при questionnaire-requirements ≠ NIL — другой эндпоинт
//!   (localentity/withdraw/apply), здесь не реализован — «Проверить подключение» это показывает (account.rs).
//! - GET /sapi/v1/capital/withdraw/history (вес UID 18000, «10 requests per second» — при пределе UID 180000 в минуту
//!   это ~10 в минуту) — withdrawOrderId (окно ≤ 7 дней, без дат — последние 7 дней), idList (до 45 id); status: 0 письмо
//!   отправлено, 2 ждёт подтверждения, 3 отклонён, 4 в обработке, 6 завершён (с withdrawOrderId и startTime/endTime —
//!   окно меньше 7 дней); 1 и 5 в документации не описаны — не
//!   угадываем (BadResponse с записью). Поля: id, amount, transactionFee, coin, network, txId, info («failure reason»).

use std::sync::LazyLock;
use std::time::Duration;

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::core::types::{Asset, Cost, CostKind, Transfer, TransferStatus, WithdrawRequest};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, rest};

/// Длина withdrawOrderId — предела в документации нет; буквы и цифры, до 32 (как у других бирж).
const MAX_WITHDRAW_ORDER_ID: usize = 32;
/// Путь истории выводов.
const HISTORY: &str = "/sapi/v1/capital/withdraw/history";
/// Когда история выводов запрошена последний раз (один адаптер Binance на процесс).
static LAST_HISTORY: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// Код walletType по venue.withdraw_wallet. Ошибка NotConfigured — не задана или не spot/funding.
fn wallet_type(ctx: &Ctx) -> GatewayResult<&'static str> {
    match ctx.text("venue.withdraw_wallet")?.as_str() {
        "spot" => Ok("0"),
        "funding" => Ok("1"),
        other => Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.withdraw_wallet",
            format!("venue.withdraw_wallet = {other}: у Binance spot или funding (walletType 0/1)"),
        ))),
    }
}

/// Вывод. Принимает: запрос ядра (сеть — код network). Возвращает: перевод Pending с id Binance.
/// Ошибки: настройки, ключи, отказ Binance (WithdrawRejected с msg — например −4035 «адрес не в белом списке»),
/// OutcomeUnknown — ответа нет и вывод не найден по withdrawOrderId.
pub async fn withdraw(ctx: &Ctx, req: WithdrawRequest) -> GatewayResult<Transfer> {
    let wallet = wallet_type(ctx)?;
    ctx.int("venue.withdraw_history_min_ms")?;
    if req.deal_id.is_empty() || req.amount <= rust_decimal::Decimal::ZERO {
        return Err(ctx.tag(GatewayError::new(
            ErrorCode::WithdrawRejected,
            None,
            format!(
                "Вывод не отправлен: пустой ID сделки или сумма {} не больше нуля",
                req.amount
            ),
        )));
    }
    // Свой id — новый на каждую попытку (метка попытки в начале, решение 114 / А-🟠5).
    let attempt = Utc::now().timestamp_millis().rem_euclid(1_000_000_000);
    let order_id: String = format!("w{attempt:09}{}", req.deal_id)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_WITHDRAW_ORDER_ID)
        .collect();
    let params = vec![
        ("coin", req.asset.ticker.clone()),
        ("network", req.network.clone()),
        ("address", req.address.clone()),
        ("amount", req.amount.normalize().to_string()),
        ("withdrawOrderId", order_id.clone()),
        ("walletType", wallet.to_string()),
    ];
    let path = "/sapi/v1/capital/withdraw/apply";
    let sent_at = Utc::now().timestamp_millis();
    let account = super::withdraw_diag::mark(ctx);
    let snap = super::withdraw_diag::snapshot(account, wallet, &req.asset.ticker);
    let epoch = crate::gateway::addr_book::key_epoch();
    // Расчётный круг Base (контракт supports_settlement_withdrawal): свой withdrawOrderId сохраняется ядром ДО отправки
    // (gateway/settlement_intent.rs before_send); после потери ответа вывод ищется по нему (settle.rs resolve_withdraw,
    // история выводов с withdrawOrderId). Вне расчётного круга — как раньше, без записи.
    let settling = crate::gateway::settlement_intent::active();
    let lookup = PendingRef::withdraw(&order_id);
    if settling {
        crate::gateway::settlement_intent::before_send(&lookup).await?;
    }
    let sent = rest::signed(ctx, Method::POST, path, params, Op::Withdraw)
        .await
        .map_err(|e|{
            let code=errors::code_of(&e);
            let mut e=not_ready(ctx,&req.asset.ticker,e);
            if matches!(code,Some(-4026 | -4024)){
                let snap=super::withdraw_diag::describe(snap.filter(|_|epoch==crate::gateway::addr_book::key_epoch()),wallet,sent_at);
                e.message.push_str(&format!("; запрос {order_id}: {} {} в сети {}, walletType={wallet}, отправка t={sent_at}, ответ t={}; {snap}. Доступное именно для вывода Binance не сообщила.",req.amount.normalize(),req.asset.ticker,req.network,Utc::now().timestamp_millis()));
            }
            e
        });
    let sent = sent.and_then(|d| {
        num::text_opt(&d, "id").ok_or_else(|| {
            let msg = format!("Вывод {order_id}: ответ без id — итог неизвестен");
            ctx.tag(GatewayError::new(ErrorCode::OutcomeUnknown, None, msg))
                .with_raw(d.to_string())
        })
    });
    let venue_ref = match sent {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match find_own(ctx, &order_id).await.ok().flatten() {
                Some(rec) => match num::text_opt(&rec, "id") {
                    Some(id) => id,
                    None => return Err(with_lookup(settling, e, &lookup)),
                },
                None => return Err(with_lookup(settling, e, &lookup)),
            }
        }
        Err(e) => return Err(e),
    };
    Ok(Transfer {
        venue: ctx.venue(),
        asset: req.asset,
        network: req.network,
        amount: req.amount,
        status: TransferStatus::Pending,
        venue_ref,
        tx_hash: None,
        fee: None,
        updated_at: Utc::now(),
    })
}

/// Решение 253: отказ вывода −4026 CAPITAL_WITHDRAW_USER_ASSET_NOT_ENOUGH («You have insufficient balance») или −4024
/// CAPITAL_WITHDRAW_USER_NO_ASSET («You don't have this asset») — Wallet → Error Code
/// (https://developers.binance.com/en/docs/products/wallet/error-code) — в пределах venue.withdraw_unready_window_ms
/// после своей покупки этой монеты (withdraw_ready.rs since_trade): «монета ещё не доступна для вывода» — WithdrawLocked
/// (вывод не создан, повтор безопасен; повтор — core/transfer_locked.rs по transfer.locked_retry_ms /
/// transfer.locked_retries_max площадки, как −4060, решение 166). Живые факты 10.11: 12 сделок, отказ через 0.35–0.77 с
/// после покупки. Дополнение t89 (Тигран 11.11): окно — от своей сделки по монете, покупки ИЛИ продажи (выручка USDT:
/// сделка 20261111122340181OPEN — продажа 12:24:21.980, возврат USDT отказан −4026 через 0.6 с).
/// Окно не задано, своей сделки не было или она раньше окна — ошибка как есть (WithdrawRejected).
/// Принимает: окружение, монету (тикер Binance), ошибку вывода. Возвращает: ошибку. Ошибок нет.
pub fn not_ready(ctx: &Ctx, ticker: &str, e: GatewayError) -> GatewayError {
    if !matches!(errors::code_of(&e), Some(-4026 | -4024)) {
        return e;
    }
    let Ok(window) = ctx.int("venue.withdraw_unready_window_ms") else {
        return e;
    };
    match super::withdraw_ready::since_trade(ticker) {
        Some(t) if t <= Duration::from_millis(window) => GatewayError {
            code: ErrorCode::WithdrawLocked,
            message: format!(
                "{} — своя сделка по {ticker} {} мс назад: ещё не доступна для вывода (решение 253)",
                e.message,
                t.as_millis()
            ),
            ..e
        },
        _ => e,
    }
}

/// История выводов не чаще venue.withdraw_history_min_ms (ждёт свою очередь). Ошибки: настройка, связь, коды, формат.
async fn history(ctx: &Ctx, p: rest::Params) -> GatewayResult<Vec<Value>> {
    let gap = Duration::from_millis(ctx.int("venue.withdraw_history_min_ms")?);
    let mut last = LAST_HISTORY.lock().await;
    if let Some(at) = *last {
        tokio::time::sleep_until(at + gap).await;
    }
    *last = Some(Instant::now());
    let data = rest::signed(ctx, Method::GET, HISTORY, p, Op::Other).await?;
    data.as_array()
        .cloned()
        .ok_or_else(|| errors::bad("withdraw/history не массив", &data.to_string()))
}

/// Что делает: своя запись вывода по withdrawOrderId (Withdraw History: «client side id for withdrawal, if provided in
/// POST /sapi/v1/capital/withdraw/apply, can be used here for query»; без startTime/endTime — «last 7 days records»).
/// Запись берётся только с тем же withdrawOrderId в ответе (поле «Returned only if one was set») — не «первая в
/// списке». Принимает: окружение, свой ID. Возвращает: Some(запись); None — записи с этим ID нет.
/// Ошибки: как у history (настройка, связь, коды, формат).
pub(super) async fn find_own(ctx: &Ctx, order_id: &str) -> GatewayResult<Option<Value>> {
    let list = history(ctx, vec![("withdrawOrderId", order_id.to_string())]).await?;
    Ok(own_record(&list, order_id).cloned())
}

/// Запись истории выводов с этим withdrawOrderId. Принимает: записи, свой ID. Возвращает: запись или None. Ошибок нет.
pub fn own_record<'a>(list: &'a [Value], order_id: &str) -> Option<&'a Value> {
    list.iter()
        .find(|r| num::text_opt(r, "withdrawOrderId").as_deref() == Some(order_id))
}

/// Что делает: в расчётном круге прикладывает к ошибке «итог неизвестен» свой withdrawOrderId (решение 231,
/// gateway/pending.rs). Вне круга ошибка не меняется. Принимает: признак круга, ошибку, «чем найти».
/// Возвращает: ошибку. Ошибок нет.
fn with_lookup(settling: bool, e: GatewayError, lookup: &PendingRef) -> GatewayError {
    if settling && e.pending.is_none() {
        e.pending(lookup.clone())
    } else {
        e
    }
}

/// Статус вывода по id Binance (idList). 0, 2, 4 → Pending; 6 → Sent; 3 → отказ с info; иное — BadResponse.
/// Ошибки: не найден, связь, неописанный статус.
pub async fn withdrawal_status(ctx: &Ctx, id: &str) -> GatewayResult<Transfer> {
    let list = history(ctx, vec![("idList", id.to_string())]).await?;
    let rec = list
        .iter()
        .find(|r| num::text_opt(r, "id").as_deref() == Some(id))
        .ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("Вывод {id} не найден в истории Binance"),
            ))
        })?;
    let status = match num::int(rec, "status")? {
        0 | 2 | 4 => TransferStatus::Pending,
        6 => TransferStatus::Sent,
        3 => {
            let info = num::text_opt(rec, "info").unwrap_or_default();
            return Err(ctx.tag(
                GatewayError::new(
                    ErrorCode::WithdrawRejected,
                    None,
                    format!("Binance: вывод {id} отклонён (status 3) {info}"),
                )
                .with_raw(rec.to_string()),
            ));
        }
        other => {
            return Err(errors::bad(
                &format!(
                    "статус вывода {other} не описан в документации Binance (0, 2, 3, 4, 6) — проверить вручную"
                ),
                &rec.to_string(),
            ));
        }
    };
    transfer(ctx, rec, id, status)
}

/// История Binance не содержит адрес токена. Если собственная заявка и окончательная
/// запись совпали и сумма из истории уже за вычетом комиссии, можно привязать
/// контракт к локальному тикеру комиссии (в том числе у лота).
pub(super) fn bind_net_withdrawal(created: &Transfer, sent: &mut Transfer) {
    let Some(fee) = sent.fee.as_ref() else { return };
    if created.asset.contract.is_some()
        && created.asset.chain.is_some()
        && created.venue == sent.venue
        && !created.venue_ref.is_empty()
        && created.venue_ref == sent.venue_ref
        && !created.network.is_empty()
        && created.network == sent.network
        && sent.status == TransferStatus::Sent
        && sent.tx_hash.as_ref().is_some_and(|h| !h.is_empty())
        && created.tx_hash.as_ref().is_none_or(|old| {
            sent.tx_hash
                .as_ref()
                .is_some_and(|h| crate::gateway::tx_hash::same(old, h))
        })
        && sent.asset.contract.is_none()
        && sent.asset.chain.is_none()
        && sent.asset.ticker == created.asset.ticker
        && fee.asset == sent.asset.ticker
        && fee.kind == CostKind::WithdrawFee
        && !fee.in_price
        && fee.paid_in.is_none()
        && fee.amount > rust_decimal::Decimal::ZERO
        && sent.amount.checked_add(fee.amount) == Some(created.amount)
    {
        sent.asset = created.asset.clone();
    }
}

/// Перевод формата ядра из записи истории (вывода или депозита). Ошибки: нет coin/amount.
pub fn transfer(
    ctx: &Ctx,
    rec: &Value,
    venue_ref: &str,
    status: TransferStatus,
) -> GatewayResult<Transfer> {
    let ticker = num::text(rec, "coin")?;
    let fee = num::dec_opt(rec, "transactionFee")?
        .map(|f| Cost::new(CostKind::WithdrawFee, f, ticker.clone(), false));
    Ok(Transfer {
        venue: ctx.venue(),
        asset: Asset {
            chain: None,
            ticker,
            contract: None,
        },
        network: num::text_opt(rec, "network").unwrap_or_default(),
        amount: num::dec(rec, "amount")?,
        status,
        venue_ref: venue_ref.to_string(),
        tx_hash: num::text_opt(rec, "txId"),
        fee,
        updated_at: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ChainId;
    use serde_json::json;

    #[test]
    fn bind_only_when_own_gross_proves_fee_in_net_history() {
        let rows: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/data/binance_docs_2026-11-10/withdraw_history.json"
        ))
        .unwrap();
        let row = &rows[0];
        let history: Transfer = serde_json::from_value(json!({
            "venue": "binance", "asset": {"ticker":row["coin"], "bsc_contract":null},
            "network":row["network"], "amount":row["amount"], "status":"Sent",
            "venue_ref":row["id"], "tx_hash":row["txId"], "updated_at":"2023-03-23T16:52:41Z",
            "fee":{"kind":"WithdrawFee","asset":row["coin"],"amount":row["transactionFee"],"in_price":false,"step":null}
        })).unwrap();
        let mut created = history.clone();
        created.status = TransferStatus::Pending;
        created.tx_hash = None;
        created.asset.chain = Some(ChainId("ethereum".into()));
        created.asset.contract = Some(
            "0xdAC17F958D2ee523a2206206994597C13D831ec7"
                .parse()
                .unwrap(),
        );
        created.amount += history.fee.as_ref().unwrap().amount;
        let mut sent = history.clone();
        bind_net_withdrawal(&created, &mut sent);
        assert_eq!(sent.asset, created.asset);
        assert_eq!(sent.amount, history.amount);
        let mut other = history.clone();
        other.network = "BSC".into();
        bind_net_withdrawal(&created, &mut other);
        assert!(other.asset.contract.is_none());
        let mut uncertain = history.clone();
        created.amount = history.amount;
        bind_net_withdrawal(&created, &mut uncertain);
        assert!(uncertain.asset.contract.is_none());
    }
}
