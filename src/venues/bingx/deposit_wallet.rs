//! Файл: venues/bingx/deposit_wallet.rs
//! Что делает: на какой счёт BingX зачисляет депозит монеты. Счёт зачисления выбирается в аккаунте BingX отдельно по
//! каждой монете (Тигран, 28.11.2026: Spot — только у USDT и USDC, остальные монеты — на фонд). Запоминает walletType
//! адреса депозита в момент, когда адрес читается из API (deposit_addr.rs), под отпечатком ключа API, и по этой памяти
//! отвечает:
//! - Venue::supports_settlement_credit (credit_ready) — у адреса расчётной монеты в сети venue.base_network (монета —
//!   по контракту dex.settlement_contract, её находит deposit_wallet_warm.rs) walletType есть и не 1/2/3;
//! - Venue::deposit_to_trading (direct_to_spot, fund_move.rs) — монета зачисляется сразу на спот: перевод фонд → спот
//!   не нужен.
//!
//! Кто вызывает: deposit_addr.rs (keep — после ответа API), deposit_wallet_warm.rs (set_targets), mod.rs
//! (credit_ready), fund_move.rs (direct_to_spot).
//! От чего зависит: ctx.rs (настройки, хранилище секретов), num.rs.
//! Настройки: venue.base_network [bingx] (существующая; новых нет).
//!
//! Официальная документация: docs-v3 (https://bingx-api.github.io/docs-v3/), Account and Wallet → «Main Account Deposit
//! Address», GET /openApi/wallets/v1/capital/deposit/address — поле ответа walletType (int64). Английский текст:
//! «Account type, 1 Fund Account; 2 Standard Futures Account; 3 Perpetual Futures Account; 4 Spot Account»; китайский
//! текст той же страницы: «1 資金賬戶；2 標合賬戶；3 永續賬戶；15 現貨賬戶». Версии совпадают в 1, 2, 3 и расходятся в
//! коде спота (4 / 15) — на код спота здесь не опираемся: зачисление «не на торговый» = 1, 2 или 3.
//! Журнал изменений docs-v3 2025-07-11: «Added wallType field to responses for 'Main Account Deposit Address' …».
//! Решения: 102 (депозит приходил на фондовый счёт, перевод fund_move.rs), Тигран 28.11.2026 (счёт зачисления — по
//! монете; USDT и USDC — Spot).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::ctx::{Ctx, SECRET_API_KEY};
use super::num;

/// Коды walletType, одинаковые в английской и китайской версиях документации: фондовый и два фьючерсных счёта.
/// Депозит на такой счёт — не на торговый (спотовый).
const NOT_TRADING: [i64; 3] = [1, 2, 3];

/// Ключ памяти: (отпечаток ключа API, монета, сеть).
type Key = (u64, String, String);

/// walletType прочитанных адресов депозита: ключ → код (None — поля нет) и расчётные монеты (монета, сеть) из
/// последнего справочника. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct WalletTypes {
    seen: Arc<Mutex<HashMap<Key, Option<i64>>>>,
    targets: Arc<Mutex<Vec<(String, String)>>>,
}

/// Код зачисляет на торговый счёт: поле есть и не из NOT_TRADING.
fn trading(code: Option<i64>) -> bool {
    code.is_some_and(|c| !NOT_TRADING.contains(&c))
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

    /// Что делает: заменяет список расчётных монет (монета BingX, сеть) — по последнему справочнику сетей.
    /// Принимает: список. Ошибок нет.
    pub fn set_targets(&self, list: Vec<(String, String)>) {
        *self.targets.lock().unwrap_or_else(|p| p.into_inner()) = list;
    }

    /// Что делает: адрес каждой расчётной монеты сети прочитан под ключом `mark` и зачисляет на торговый счёт.
    /// Принимает: отпечаток ключа, сеть. Возвращает: true — расчётная монета в этой сети есть и у каждой walletType
    /// есть и не из NOT_TRADING; нет ключа, нет монеты, адрес не читался, нет поля или 1/2/3 — false. Ошибок нет.
    pub fn trading_credit(&self, mark: Option<u64>, network: &str) -> bool {
        let Some(m) = mark else { return false };
        let targets = self
            .targets
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let mut mine = targets.iter().filter(|(_, n)| n == network).peekable();
        mine.peek().is_some()
            && mine.all(|(c, n)| trading(seen.get(&(m, c.clone(), n.clone())).copied().flatten()))
    }

    /// Что делает: монета зачисляется сразу на торговый счёт — все прочитанные под ключом `mark` адреса этой монеты
    /// (в любых сетях: deposit_to_trading сеть не получает) имеют walletType не из NOT_TRADING. Принимает: отпечаток
    /// ключа, монету. Возвращает: true — есть хотя бы один адрес и все такие; нет ключа, адрес не читался, нет поля или
    /// хоть один 1/2/3 — false (прежнее поведение перевода). Ошибок нет.
    pub fn direct_to_spot(&self, mark: Option<u64>, coin: &str) -> bool {
        let Some(m) = mark else { return false };
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let mut codes = seen
            .iter()
            .filter(|((k, c, _), _)| *k == m && c == coin)
            .map(|(_, code)| *code)
            .peekable();
        codes.peek().is_some() && codes.all(trading)
    }
}

/// Что делает: условие Venue::supports_settlement_credit. Принимает: окружение, память walletType.
/// Возвращает: true — задана venue.base_network и trading_credit по ней под текущим ключом API; иначе false.
/// Ошибок нет.
pub fn credit_ready(ctx: &Ctx, types: &WalletTypes) -> bool {
    let Ok(network) = ctx.text("venue.base_network") else {
        return false;
    };
    types.trading_credit(ctx.vault.mark(SECRET_API_KEY), &network)
}
