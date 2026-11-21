//! Файл: venues/okx/deposit_addr.rs
//! Что делает: адрес депозита OKX по монете и сети и счёт, на который он зачисляет (6 — Funding, 18 — Trading). Кэш в
//! памяти на время процесса (адрес нужен в предпроверке каждой сделки — решение 118, урок 25). Адрес с tag/memo/pmtId —
//! ошибка «требует memo — не поддерживается» (вывод на него без memo не зачислится); такой не кэшируется.
//! Адрес для перевода на OKX (for_deposit) — с зачислением на Trading (18): монету сразу можно продать. Адрес только
//! Funding (6) принимается при venue.deposit_to_spot = да (после зачисления — перевод 6 → 18, fund_move.rs).
//! Кто вызывает: mod.rs (deposit_address), fund_move.rs (на какой счёт приходит депозит).
//! От чего зависит: rest.rs, num.rs, wallet.rs (chain OKX), ctx.rs.
//! Настройки: venue.deposit_to_spot [okx] (да — допускается адрес с зачислением на Funding).
//!
//! Официально — «Funding Account → REST API → Get deposit address» GET /api/v5/asset/deposit-address?ccy= (6 запросов/с,
//! User ID) https://www.okx.com/docs-v5/en/: addr, tag / memo / pmtId («returned only when the currency needs them»),
//! chain, to («The beneficiary account 6: Funding account 18: Trading account»; «The users under some entity (e.g.
//! Brazil) only support deposit to trading account»), selected («the address is selected on the website»), ctAddr («Last
//! 6 digits of contract address» — для сопоставления не годится).
//! Решение 379 (задача t165, 17.11.2026): страницу docs-v5 в этой задаче прочитать не удалось (доступ к сайту не дан) —
//! есть ли метод создания адреса, по документации НЕ установлено (в копии SDK python-okx 0.4.4, okx/consts.py, из
//! адресов депозита только GET /api/v5/asset/deposit-address — это не документация). Поведение прежнее: нет адреса —
//! отказ (проверить docs-v5 — docs/TASKS.md, t165).

use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::addr_book::{AddrBook, Mark};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::{num, rest, wallet};

/// Счета OKX — значения from / to документации («6: Funding account», «18: Trading account»).
pub const FUNDING: &str = "6";
pub const TRADING: &str = "18";

/// Адрес депозита и счёт зачисления («6» или «18», как в ответе).
#[derive(Debug, Clone)]
pub struct DepositAddr {
    pub address: String,
    pub to: String,
}

/// Полученные адреса: (монета, сеть ядра) → адрес. Дешёво клонируется.
/// Решение 411: ключ памяти — ещё и отпечаток ключа API биржи (смена ключа — старый адрес не берётся).
#[derive(Clone, Default)]
pub struct DepositAddrs {
    book: AddrBook<DepositAddr>,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти или запросом.
    /// Принимает: окружение, монету, сеть ядра. Возвращает: DepositAddr.
    /// Ошибки: InvalidAddress — адреса в сети нет (создать в кабинете OKX) или он требует memo; связь, коды OKX.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<DepositAddr> {
        // Решение 411: память — под отпечатком ключа API биржи и по режиму задачи (gateway/addr_book.rs).
        let mark = Mark::read(&ctx.vault, super::ctx::SECRET_API_KEY);
        if let Some(a) = self.book.lookup(&mark, &asset.ticker, network)? {
            return Ok(a);
        }
        let (chain, _) = wallet::chain(ctx, &asset.ticker, network).await?;
        let q = vec![("ccy", asset.ticker.clone())];
        let data = rest::get(ctx, "/api/v5/asset/deposit-address", q).await?;
        let a = pick(&data, &chain)
            .map_err(|m| invalid(ctx, format!("{} {chain}: {m}", asset.ticker)))?;
        self.book
            .keep(&mark, asset.ticker.clone(), network.to_string(), a.clone());
        Ok(a)
    }
}

/// Выбор адреса сети chain: сначала с зачислением на Trading (18), затем Funding (6); среди равных — selected = true.
/// Ошибка — текст причины (адреса нет; адрес с tag/memo/pmtId).
pub fn pick(data: &Value, chain: &str) -> Result<DepositAddr, String> {
    let rows: Vec<&Value> = data
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|r| num::text_opt(r, "chain").as_deref() == Some(chain))
                .collect()
        })
        .unwrap_or_default();
    let rank = |r: &&Value| {
        let to = num::text_opt(r, "to").unwrap_or_default();
        let sel = matches!(r.get("selected"), Some(Value::Bool(true)));
        (to != TRADING, !sel)
    };
    let best = rows
        .iter()
        .min_by_key(|r| rank(r))
        .ok_or("адреса депозита нет — создайте его в кабинете OKX (Deposit)")?;
    for f in ["tag", "memo", "pmtId"] {
        if let Some(v) = num::text_opt(best, f).filter(|v| !v.trim().is_empty()) {
            return Err(format!("адрес требует {f} ({v}) — не поддерживается"));
        }
    }
    Ok(DepositAddr {
        address: num::text_opt(best, "addr").ok_or("в ответе нет addr")?,
        to: num::text_opt(best, "to").unwrap_or_default(),
    })
}

/// Что делает: адрес, на который ядро переводит монету на OKX: с зачислением на Trading (18); Funding (6) — только при
/// venue.deposit_to_spot = да. Принимает: адреса, окружение, монету, сеть ядра. Возвращает: адрес.
/// Ошибки: как DepositAddrs::get; InvalidAddress — адрес зачисляет на Funding, а перевод 6 → 18 выключен.
pub async fn for_deposit(
    addrs: &DepositAddrs,
    ctx: &Ctx,
    asset: &Asset,
    network: &str,
) -> GatewayResult<String> {
    let a = addrs.get(ctx, asset, network).await?;
    if a.to != TRADING && !ctx.flag("venue.deposit_to_spot").unwrap_or(false) {
        let m = format!(
            "Адрес депозита OKX {} в сети {network} зачисляет на счёт {} (не Trading 18): монету не продать без перевода — \
             выберите в кабинете OKX зачисление на Trading account или включите venue.deposit_to_spot",
            asset.ticker, a.to
        );
        return Err(invalid(ctx, m));
    }
    Ok(a.address)
}

/// Ошибка InvalidAddress с площадкой OKX.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
