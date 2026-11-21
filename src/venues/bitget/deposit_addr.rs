//! Файл: venues/bitget/deposit_addr.rs
//! Что делает: адрес депозита Bitget по монете и сети (chain). Кэш в памяти на время процесса (адрес нужен в
//! предпроверке каждой сделки до покупки — урок 25, решение 118). Адрес с непустым tag — ошибка «требует tag — не
//! поддерживается» (вывод на него без tag не зачислится); такой не кэшируется.
//! Кто вызывает: mod.rs (deposit_address).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально — Get Deposit Address GET /api/v2/spot/wallet/deposit-address?coin=&chain= («10 times/1s (User ID)»)
//! https://www.bitget.com/docs/catalog/classic-spot-account/classic-spot-account#get-deposit-address — coin*, chain
//! («All coin names can be returned from Get Coin Info interface»); ответ — объект {address, chain, coin, tag, url}.
//! На какой счёт зачисляется депозит, ответ не говорит (см. fund_move.rs: Modify Deposit Account / перевод p2p → spot).
//! Решение 379 (задача t165, 17.11.2026): метода создания адреса депозита в документации нет (на странице — Get Deposit
//! Address, Get SubAccount Deposit Address, Modify Deposit Account, записи депозитов); создаёт ли GET адрес, документация
//! не говорит. Поведение прежнее: нет адреса — отказ (вопрос Тиграну — docs/TASKS.md, t165).

use reqwest::Method;

use crate::core::types::Asset;
use crate::gateway::addr_book::{AddrBook, Mark};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Полученные адреса: (монета, chain) → адрес. Дешёво клонируется.
/// Решение 411: ключ памяти — ещё и отпечаток ключа API биржи (смена ключа — старый адрес не берётся).
#[derive(Clone, Default)]
pub struct DepositAddrs {
    book: AddrBook<String>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, chain. Возвращает: адрес.
    /// Ошибки: InvalidAddress — адреса нет или адрес требует tag; связь, коды Bitget.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, chain: &str) -> GatewayResult<String> {
        // Решение 411: память — под отпечатком ключа API биржи и по режиму задачи (gateway/addr_book.rs).
        let mark = Mark::read(&ctx.vault, super::ctx::SECRET_API_KEY);
        if let Some(a) = self.book.lookup(&mark, &asset.ticker, chain)? {
            return Ok(a);
        }
        let q = vec![("coin", asset.ticker.clone()), ("chain", chain.to_string())];
        let path = "/api/v2/spot/wallet/deposit-address";
        let data = rest::signed(ctx, Method::GET, path, q, None, Op::Other).await?;
        let raw = data.to_string();
        // Поле chain ответа с параметром не сверяется: в примере документации оно «BTC-Bitcoin», а в Get Coin Info
        // сеть «BTC» — формат разный (проверить на живом ответе).
        if let Some(tag) = num::text_opt(&data, "tag").filter(|t| !t.trim().is_empty()) {
            let m = format!(
                "Адрес депозита Bitget {} в сети {chain} требует tag ({tag}) — не поддерживается",
                asset.ticker
            );
            return Err(invalid(ctx, m).with_raw(raw));
        }
        let address = num::text_opt(&data, "address").ok_or_else(|| {
            let m = format!(
                "У {} нет адреса депозита Bitget в сети {chain} — проверьте в кабинете Bitget (Deposit)",
                asset.ticker
            );
            invalid(ctx, m).with_raw(raw.clone())
        })?;
        self.book.keep(
            &mark,
            asset.ticker.clone(),
            chain.to_string(),
            address.clone(),
        );
        Ok(address)
    }
}

/// Ошибка InvalidAddress с площадкой Bitget.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
