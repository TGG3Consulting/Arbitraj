//! Файл: venues/poloniex/deposit_addr_new.rs
//! Что делает: решение 379 (задача t165, Тигран 17.11: «на всех биржах этот адрес должен автоматом создаваться») —
//! создание адреса депозита Poloniex, когда в ответе Deposit Addresses его нет, и общая проверка адреса из ответа
//! (одна и та же для GET /wallets/addresses и POST /wallets/address). Живой факт 17.11: ESIM bsc_dex → poloniex —
//! 94 отказа предпроверки подряд «нет адреса депозита … создайте адрес в кабинете Poloniex», сделки потеряны.
//! Кто вызывает: deposit_addr.rs (DepositAddrs::get), тесты (pick — tests/poloniex_deposit_addr_t165.rs).
//! От чего зависит: rest.rs, num.rs, ctx.rs (журнал), storage/journal.rs.
//! Настройки: нет своих (запрет создания у прогрева — gateway/addr_claim.rs, venue.deposit_warm_create читает ядро).
//!
//! Официально — Wallets → «New Currency Address» POST /wallets/address
//! (https://api-docs.poloniex.com/spot/api/private/wallet, прочитано 17.11.2026): «Create a new address for a
//! currency»; тело {"currency": …} — единственный параметр, обязателен («the currency to use for the deposit
//! address»); ответ {"address": …} («the newly created address»). Про сеть метод молчит — передаётся то же имя монеты
//! сети (поле coin сети в /v2/currencies, networks_parse::chain_coin), что и в GET /wallets/addresses. Лимит запросов,
//! право ключа и что будет при повторном создании (новый адрес или прежний) в этом разделе НЕ описаны — не додумываем:
//! адрес создаётся только когда GET его не дал, после создания он кэшируется (deposit_addr.rs) и больше не создаётся.
//! Записанного живого ответа POST нет: тест разбора — на примере из документации
//! (tests/data/poloniex_doc_2026-11-17); живой ответ — в сыром журнале сервера (<data-dir>/raw/poloniex/*.jsonl.zst,
//! соединение «rest /wallets/address») при первом создании — записать в tests/data (docs/TASKS.md).

use reqwest::Method;
use serde_json::{Value, json};

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

/// Что делает: создать адрес депозита (New Currency Address) и вернуть его.
/// Принимает: окружение, тикер монеты, сеть (blockchain), имя монеты сети (coin из /v2/currencies).
/// Возвращает: адрес из ответа. После создания — запись журнала venue.deposit_address (Info, без самого адреса — как
/// KuCoin, решение 208).
/// Ошибки: код и сообщение Poloniex дословно (через rest::signed → reply.rs) с пояснением, что не удалось создать;
/// InvalidAddress — в ответе нет адреса (с сырым ответом).
pub async fn create(ctx: &Ctx, ticker: &str, chain: &str, coin: &str) -> GatewayResult<String> {
    let body = json!({ "currency": coin });
    let path = "/wallets/address";
    let v = rest::signed(ctx, Method::POST, path, vec![], Some(body), Op::Other)
        .await
        .map_err(|mut e| {
            e.message = format!(
                "Poloniex отказал в создании адреса депозита {ticker} в сети {chain} (монета сети {coin}, \
                 POST {path}): {}",
                e.message
            );
            e
        })?;
    let Some(address) = pick(&v, ADDRESS) else {
        let m = format!(
            "Poloniex не вернул созданный адрес депозита {ticker} в сети {chain} (монета сети {coin}): в ответе \
             POST {path} нет поля {ADDRESS}"
        );
        let e = GatewayError::new(ErrorCode::InvalidAddress, None, m).with_raw(v.to_string());
        return Err(ctx.tag(e));
    };
    let m = format!("Создан адрес депозита Poloniex {ticker} в сети {chain} (монета сети {coin})");
    ctx.log(Level::Info, "venue.deposit_address", m).await;
    Ok(address)
}
