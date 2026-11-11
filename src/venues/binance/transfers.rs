//! Файл: venues/binance/transfers.rs
//! Что делает: вывод с Binance и его статус; поиск вывода по своему withdrawOrderId при неизвестном итоге. Запрос
//! истории выводов очень тяжёлый (вес UID 18000) — между запросами не меньше venue.withdraw_history_min_ms.
//! Кто вызывает: mod.rs (функции контракта), deposits.rs (transfer).
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
//!   отправлено, 2 ждёт подтверждения, 3 отклонён, 4 в обработке, 6 завершён; 1 и 5 в документации не описаны — не
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
    let sent = rest::signed(ctx, Method::POST, path, params, Op::Withdraw)
        .await
        .map_err(|e| not_ready(ctx, &req.asset.ticker, e));
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
            let p = vec![("withdrawOrderId", order_id.clone())];
            match history(ctx, p).await.ok().and_then(|l| first_id(&l)) {
                Some(id) => id,
                None => return Err(e),
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

/// id первой записи. Ошибок нет.
fn first_id(list: &[Value]) -> Option<String> {
    list.first().and_then(|r| num::text_opt(r, "id"))
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
            ticker,
            bsc_contract: None,
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
