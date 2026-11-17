//! Файл: venues/poloniex/deposit_addr_new.rs
//! Что делает: решение 379 (задача t165, Тигран 17.11: «на всех биржах этот адрес должен автоматом создаваться») —
//! запрос списка адресов (GET /wallets/addresses) и создание адреса депозита Poloniex (POST /wallets/address), когда
//! в списке его нет; общая проверка адреса из ответа (`pick`). Живой факт 17.11: ESIM bsc_dex → poloniex — 94 отказа
//! предпроверки подряд «нет адреса депозита … создайте адрес в кабинете Poloniex», сделки потеряны.
//! Правки по ревью t165:
//! - Б1: адрес после создания берётся ТОЛЬКО из повторного GET /wallets/addresses того же имени монеты сети (в ответе
//!   POST нет ни сети, ни монеты — связь «адрес ↔ монета сети» есть только в GET); ответ POST лишь сверяется
//!   (gateway/addr_create.rs::confirm). Нет в GET или не совпал — InvalidAddress (текст без адреса), в кэш не идёт;
//! - Н1: POST — класс операции Op::Create: на резервный адрес REST только если запрос точно не отправлен (как ордер и
//!   вывод); после таймаута — тот же повторный GET, повторного POST нет.
//!
//! Кто вызывает: deposit_addr.rs (DepositAddrs::get), тесты (pick — tests/poloniex_deposit_addr_t165.rs).
//! От чего зависит: rest.rs, num.rs, ctx.rs (журнал), gateway/addr_create.rs, storage/journal.rs.
//! Настройки: нет своих (запрет создания у прогрева — gateway/addr_claim.rs, venue.deposit_warm_create читает ядро).
//!
//! Официально — Wallets (https://api-docs.poloniex.com/spot/api/private/wallet, прочитано 17.11.2026): «Deposit
//! Addresses» GET /wallets/addresses?currency= — объект «имя монеты → адрес»; «New Currency Address» POST
//! /wallets/address — «Create a new address for a currency», тело {"currency": …} («the currency to use for the deposit
//! address»), ответ {"address": …} («the newly created address»). Про сеть POST молчит — передаётся то же имя монеты
//! сети (поле coin сети в /v2/currencies, networks_parse::chain_coin), что и в GET. Лимит, право ключа и повторное
//! создание в разделе НЕ описаны. Записанного живого ответа POST нет: тест разбора — на примере из документации
//! (tests/data/poloniex_doc_2026-11-17); живой ответ — в сыром журнале сервера (<data-dir>/raw/poloniex/*.jsonl.zst,
//! соединение «rest /wallets/address») при первом создании (docs/TASKS.md, П1).

use reqwest::Method;
use serde_json::{Value, json};

use crate::gateway::addr_create::{Confirm, Failed, Failure, confirm, post_failure};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Поле адреса в ответе New Currency Address (документация: «address — the newly created address»).
pub const ADDRESS: &str = "address";

/// Что делает: адрес из поля ответа — непустая строка. Одна проверка для обоих ответов: GET /wallets/addresses
/// (поле — имя монеты сети) и POST /wallets/address (поле address). Сеть с memo (PAYMENT_ID) отсекается раньше, по
/// справочнику (deposit_addr.rs), — до запроса адреса.
/// Принимает: ответ, имя поля. Возвращает: адрес или None (нет поля, не строка, пусто). Ошибок нет.
pub fn pick(v: &Value, field: &str) -> Option<String> {
    num::text_opt(v, field).filter(|a| !a.trim().is_empty())
}

/// Что делает: Deposit Addresses GET /wallets/addresses?currency=<имя монеты сети> (ссылка — в шапке).
/// Принимает: окружение, имя монеты сети. Возвращает: ответ (объект «имя → адрес»). Ошибки: связь, коды Poloniex.
pub async fn list(ctx: &Ctx, coin: &str) -> GatewayResult<Value> {
    let q = vec![("currency", coin.to_string())];
    rest::signed(ctx, Method::GET, "/wallets/addresses", q, None, Op::Other).await
}

/// Что делает: создать адрес (New Currency Address) и подтвердить его повторным GET (Б1, Н1 — см. шапку).
/// Принимает: окружение, тикер монеты, сеть (blockchain), имя монеты сети (coin из /v2/currencies).
/// Возвращает: адрес из GET. После подтверждения — журнал venue.deposit_address (Info, без адреса — как KuCoin,
/// решение 208); расхождение POST и GET — журнал venue.deposit_address (Warn, без адресов).
/// Ошибки (Failed — ошибка и вид, ревью 2 Н-А): отказ POST (код и сообщение Poloniex дословно) — Refused; итог POST
/// неизвестен и GET адреса не дал, POST принят, а GET адреса не дал, адреса POST и GET разные — Sent (биржа, возможно,
/// уже создала адрес); ошибка повторного GET — по виду ошибки POST (принятый POST — Sent).
pub async fn create(ctx: &Ctx, ticker: &str, chain: &str, coin: &str) -> Result<String, Failed> {
    let path = "/wallets/address";
    let body = json!({ "currency": coin });
    let posted = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Create)
        .await
        .map(|v| pick(&v, ADDRESS))
        .map_err(|mut e| {
            e.message = format!(
                "Poloniex: запрос создания адреса депозита {ticker} в сети {chain} (монета сети {coin}, POST {path}) \
                 не удался: {}",
                e.message
            );
            e
        });
    // Вид неудачи, если адреса не будет: POST принят — Sent; ошибка POST — по её коду.
    let kind = match &posted {
        Ok(_) => Failure::Sent,
        Err(e) => post_failure(&e.code),
    };
    let got = list(ctx, coin).await.map_err(|err| Failed {
        err: Box::new(err),
        kind,
    })?;
    let listed = pick(&got, coin);
    let from_post = posted.as_ref().ok().and_then(|a| a.as_deref());
    let what = format!("Poloniex {ticker} в сети {chain} (монета сети {coin})");
    let sent = |m: String| Failed {
        err: Box::new(invalid(ctx, m)),
        kind: Failure::Sent,
    };
    match confirm(from_post, listed.as_deref()) {
        Confirm::Use(a) => {
            let m = format!(
                "Создан адрес депозита {what} — подтверждён повторным GET /wallets/addresses"
            );
            ctx.log(Level::Info, "venue.deposit_address", m).await;
            Ok(a)
        }
        Confirm::NotListed => match posted {
            Err(err) => Err(Failed {
                err: Box::new(err),
                kind,
            }),
            Ok(_) => Err(sent(format!(
                "Адрес депозита {what}: POST {path} принят, но повторный GET /wallets/addresses адреса этой монеты \
                 сети не дал — адрес не принят, сделки нет"
            ))),
        },
        Confirm::Mismatch => {
            let m = format!(
                "Адрес депозита {what}: адрес из ответа POST {path} не совпал с адресом повторного GET \
                 /wallets/addresses — адрес не принят, сделки нет"
            );
            ctx.log(Level::Warn, "venue.deposit_address", m.clone())
                .await;
            Err(sent(m))
        }
    }
}

/// Ошибка InvalidAddress с площадкой Poloniex. Ошибок нет.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
