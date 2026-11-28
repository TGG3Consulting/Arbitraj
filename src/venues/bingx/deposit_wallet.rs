//! Файл: venues/bingx/deposit_wallet.rs
//! Что делает: расчётный круг USDC Base — на какой счёт BingX зачисляет депозит. Запоминает walletType адреса депозита
//! в момент, когда адрес читается из API (deposit_addr.rs), и по этой памяти отвечает синхронному
//! Venue::supports_settlement_credit (mod.rs): зачисление прямо на торговый счёт подтверждено, только если перевод
//! фонд → спот выключен (venue.deposit_to_spot [bingx] = нет или не задана) и у адресов сети venue.base_network,
//! прочитанных под текущим ключом API, walletType есть и он не фондовый и не фьючерсный.
//! Кто вызывает: deposit_addr.rs (keep — после ответа API), mod.rs (credit_ready).
//! От чего зависит: ctx.rs (настройки, хранилище секретов), num.rs.
//! Настройки: venue.deposit_to_spot, venue.base_network (обе существующие, область bingx; новых нет).
//!
//! Официальная документация: docs-v3 (https://bingx-api.github.io/docs-v3/), Account and Wallet → «Main Account Deposit
//! Address», GET /openApi/wallets/v1/capital/deposit/address — поле ответа walletType (int64). Английский текст:
//! «Account type, 1 Fund Account; 2 Standard Futures Account; 3 Perpetual Futures Account; 4 Spot Account»; китайский
//! текст той же страницы: «1 資金賬戶；2 標合賬戶；3 永續賬戶；15 現貨賬戶». Версии совпадают в 1, 2, 3 и расходятся в
//! коде спота (4 / 15) — на код спота здесь не опираемся: зачисление «не на торговый» = 1, 2 или 3.
//! Журнал изменений docs-v3 2025-07-11: «Added wallType field to responses for 'Main Account Deposit Address' …».
//! Решения: 102 (депозит приходил на фондовый счёт, перевод fund_move.rs), Тигран 28.11.2026 — зачисление депозита
//! переключено в аккаунте BingX на спотовый счёт, перевод бота выключается venue.deposit_to_spot = нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::gateway::errors::ErrorCode;

use super::ctx::{Ctx, SECRET_API_KEY};
use super::num;

/// Коды walletType, одинаковые в английской и китайской версиях документации: фондовый и два фьючерсных счёта.
/// Депозит на такой счёт — не на торговый (спотовый).
const NOT_TRADING: [i64; 3] = [1, 2, 3];

/// Ключ памяти: (отпечаток ключа API, монета, сеть).
type Key = (u64, String, String);

/// walletType прочитанных адресов депозита: ключ → код (None — поля нет). Дешёво клонируется.
#[derive(Clone, Default)]
pub struct WalletTypes {
    seen: Arc<Mutex<HashMap<Key, Option<i64>>>>,
}

impl WalletTypes {
    /// Что делает: запоминает walletType адреса из ответа API. Принимает: отпечаток ключа API, под которым получен
    /// ответ (None — ключ сменился во время запроса: не запоминается), монету, сеть, объект адреса из data.data[].
    /// Нет поля или не целое — запоминается «нет данных» (зачисление не подтверждено). Ошибок нет.
    pub fn keep(&self, mark: Option<u64>, coin: &str, network: &str, addr: &Value) {
        let Some(m) = mark else { return };
        let code = num::int(addr, "walletType").ok();
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        seen.insert((m, coin.to_string(), network.to_string()), code);
    }

    /// Что делает: есть ли в памяти запись (даже «нет поля») для адреса монеты в сети под ключом `mark`.
    /// Принимает: отпечаток ключа, монету, сеть. Возвращает: true — адрес уже читался под этим ключом. Ошибок нет.
    pub fn known(&self, mark: Option<u64>, coin: &str, network: &str) -> bool {
        let Some(m) = mark else { return false };
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        seen.contains_key(&(m, coin.to_string(), network.to_string()))
    }

    /// Что делает: все адреса сети, прочитанные под ключом `mark`, зачисляют на торговый счёт. Принимает: отпечаток
    /// ключа, сеть. Возвращает: true — есть хотя бы один адрес, у каждого walletType есть и не из NOT_TRADING;
    /// нет ключа, нет адресов, нет поля или хоть один фондовый / фьючерсный — false. Ошибок нет.
    pub fn trading_credit(&self, mark: Option<u64>, network: &str) -> bool {
        let Some(m) = mark else { return false };
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let mut codes = seen
            .iter()
            .filter(|((k, _, n), _)| *k == m && n == network)
            .map(|(_, code)| *code)
            .peekable();
        codes.peek().is_some() && codes.all(|c| c.is_some_and(|c| !NOT_TRADING.contains(&c)))
    }
}

/// Что делает: условие Venue::supports_settlement_credit. Принимает: окружение, память walletType.
/// Возвращает: true — venue.deposit_to_spot не «да» (нет или не задана) и trading_credit по сети venue.base_network
/// под текущим ключом API; иначе (перевод фонд → спот включён, сеть Base не задана, нет данных) — false. Ошибок нет.
pub fn credit_ready(ctx: &Ctx, types: &WalletTypes) -> bool {
    let Some(network) = credit_network(ctx) else {
        return false;
    };
    types.trading_credit(ctx.vault.mark(SECRET_API_KEY), &network)
}

/// Что делает: настройки допускают прямое зачисление — venue.deposit_to_spot не «да» (нет или не задана) и задана
/// venue.base_network. Принимает: окружение. Возвращает: Some(имя сети Base у BingX); иначе None. Ошибок нет.
/// Вызывают: credit_ready, deposit_wallet_warm.rs.
pub fn credit_network(ctx: &Ctx) -> Option<String> {
    // Перевод фонд → спот включён — депозит по решению 102 ждётся на фонде; круг с ним не совместим.
    // Неразборчивое значение — тоже не «нет»: зачисление не подтверждено.
    match ctx.flag("venue.deposit_to_spot") {
        Ok(false) => {}
        Err(e) if e.code == ErrorCode::NotConfigured => {}
        _ => return None,
    }
    ctx.text("venue.base_network").ok()
}
