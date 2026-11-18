//! Файл: venues/kraken/deposit_addr.rs
//! Что делает: адрес депозита Kraken по монете и сети: сначала выданные адреса метода (список v2), нет годного —
//! новый адрес (claim) создаётся автоматически (решение 379, Тигран 17.11: «Kraken — создание адреса АВТОМАТИЧЕСКИ»;
//! настройка venue.deposit_address_claim удалена), deposit_addr_claim.rs. Кэш в памяти (адрес нужен в предпроверке
//! каждой сделки — решение 118, урок 25) с учётом expire_time. Адрес с tag/memo — ошибка «требует memo — не
//! поддерживается»; такой не кэшируется.
//! Решение 316 (доработка): под признаком задачи «без создания адреса» (gateway/addr_claim.rs — ставит фоновый прогрев
//! адресов) claim не выполняется — NotApplicable.
//! Решение 379 (Kraken): общий путь создания gateway/addr_create.rs — одно создание за раз на (монета, сеть), неудача
//! (в т. ч. TooManyDepositAddresses) помнится на venue.networks_fresh_ms (не задана — до перезапуска), адрес — только
//! из повторного списка v2 после claim, claim на резервный REST — только если запрос точно не отправлен.
//! Кто вызывает: mod.rs (deposit_address), deposit_addr_claim.rs (list, find, crypto), тесты (pick).
//! От чего зависит: rest.rs, num.rs, wallet.rs (method_id), methods.rs, ctx.rs, deposit_addr_claim.rs,
//! gateway/addr_claim.rs, gateway/addr_create.rs.
//! Настройки: venue.funding_list_limit [kraken]; venue.networks_fresh_ms [kraken] — срок памяти неудачного claim.
//!
//! Официально:
//! - GET /funding/v2/deposit/addresses (OpenAPI spot-rest.yaml, listFundingClaimedAddressesV2): scope[method_id], asset,
//!   limit 1–500, cursor; addresses[] — method_id, id, last_used, expire_time, address_details.crypto {address, tag, memo};
//! - PUT /funding/v1/deposit/address (claimFundingDepositAddressV1): тело {method_id} → address_details.crypto;
//! - гид https://docs.kraken.com/exchange/guides/rest/funding.md (прочитан 17.11.2026): «Claiming can reserve a new
//!   address, so list first and only claim when you need another one»; «Where address_generation is limited (typically
//!   limit: 5), you may hold up to five unused claimed addresses for that method at a time»; «If you are already at the
//!   limit, claim returns TooManyDepositAddresses. List existing addresses instead»; «the oldest unused address then
//!   begins expiring and is removed after 7 days»; «If tag or memo is present, include it with the address» — у бота
//!   memo не поддерживается. HTTP-статус и формат тела TooManyDepositAddresses в гиде не указаны.
//!
//! Счёт зачисления у Kraken один (спот) — перевода после депозита нет.

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::Value;

use crate::core::types::Asset;
use crate::gateway::addr_book::AddrBook;
use crate::gateway::addr_claim::{self, Missing};
use crate::gateway::addr_create::{Creating, Failure, retry_text};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::methods::{CLASS, Dir};
use super::{deposit_addr_claim, num, rest, wallet};

/// Адрес и когда он истекает (None — срок не указан).
pub type Entry = (String, Option<DateTime<Utc>>);

/// Полученные адреса: (монета, сеть ядра) → адрес; память создания (решение 379). Дешёво клонируется.
/// Решение 411: ключ памяти — ещё и отпечаток ключа API биржи (смена ключа — старый адрес не берётся).
#[derive(Clone, Default)]
pub struct DepositAddrs {
    book: AddrBook<Entry>,
    creating: Creating,
}

impl DepositAddrs {
    /// Что делает: адрес депозита монеты в сети — из памяти (не истёк), из списка v2 или новым claim.
    /// Принимает: окружение, монету, сеть ядра. Возвращает: адрес.
    /// Ошибки: InvalidAddress — адрес требует memo, claim не подтверждён списком; NotApplicable — адреса нет, а создание
    /// запрещено признаком задачи (прогрев); отказ Kraken на claim (TooManyDepositAddresses и др.) — его текст, повтор
    /// не раньше срока venue.networks_fresh_ms; связь, ответ Kraken.
    pub async fn get(&self, ctx: &Ctx, asset: &Asset, network: &str) -> GatewayResult<String> {
        let key = (asset.ticker.clone(), network.to_string());
        // Решение 411: память — под отпечатком ключа API биржи и по режиму задачи (gateway/addr_book.rs).
        // Срок адреса (expire_time) отбирается до решения по режиму.
        let mark = ctx.vault.mark(super::ctx::SECRET_API_KEY);
        if let Some(a) = self.book.apply(self.cached(mark, &key), &key.0, &key.1)? {
            return Ok(a);
        }
        let (method_id, _) = wallet::method(ctx, &asset.ticker, network, Dir::Deposit).await?;
        let v = list(ctx, &asset.ticker, &method_id).await?;
        let found =
            pick(&v).map_err(|m| invalid(ctx, format!("{} {network}: {m}", asset.ticker)))?;
        if let Some(e) = found {
            return Ok(self.keep(mark, key, e));
        }
        if addr_claim::on_missing() == Missing::Forbidden {
            let m = addr_claim::forbidden_text("Kraken", &asset.ticker, network);
            return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, m)));
        }
        // Решение 379: одно создание за раз на (монета, сеть); дождавшийся берёт итог первого (кэш или неудача).
        let _pass = self.creating.enter(&key).await;
        // Создал ли адрес тот, кого ждали (память без учёта режима: это ответ биржи, полученный сейчас).
        if let Some(a) = self.cached(mark, &key) {
            return Ok(a);
        }
        // Ревью 2 Н-А: отказ Kraken не повторяется до срока venue.networks_fresh_ms (не задана — до перезапуска),
        // claim без подтверждения списком — до перезапуска; причина видна в отказе.
        let keep = ctx
            .int("venue.networks_fresh_ms")
            .ok()
            .map(Duration::from_millis);
        if let Some(e) = self.creating.failed(&key, keep) {
            return Err(e);
        }
        match deposit_addr_claim::claim(ctx, &asset.ticker, network, &method_id).await {
            Ok(e) => {
                self.creating.ok(&key);
                Ok(self.keep(mark, key, e))
            }
            Err(f) => {
                // Одна запись Warn на ключ — claim отправлен, адреса в списке нет.
                if self.creating.fail(&key, &f) && f.kind == Failure::Sent {
                    let m = format!(
                        "Kraken {} в сети {network}: {} — {}",
                        asset.ticker,
                        f.err.message,
                        retry_text(Failure::Sent, keep)
                    );
                    ctx.log(Level::Warn, "venue.deposit_address", m).await;
                }
                Err(*f.err)
            }
        }
    }

    /// Адрес из памяти под отпечатком ключа API (решение 411), если не истёк. Ошибок нет.
    fn cached(&self, mark: Option<u64>, key: &(String, String)) -> Option<String> {
        let (a, exp) = self.book.peek(mark, &key.0, &key.1)?;
        exp.is_none_or(|e| e > Utc::now()).then_some(a)
    }

    /// Адрес — в память под отпечатком ключа API (решение 411). Возвращает: адрес. Ошибок нет.
    fn keep(&self, mark: Option<u64>, key: (String, String), e: Entry) -> String {
        let a = e.0.clone();
        self.book.keep(mark, key.0, key.1, e);
        a
    }
}

/// Что делает: выданные адреса метода — GET /funding/v2/deposit/addresses (ссылка — в шапке), одной страницей
/// venue.funding_list_limit. Принимает: окружение, тикер, method_id. Возвращает: ответ.
/// Ошибки: NotConfigured — venue.funding_list_limit не задана; связь, ответ Kraken.
pub async fn list(ctx: &Ctx, ticker: &str, method_id: &str) -> GatewayResult<Value> {
    let limit = ctx.int("venue.funding_list_limit")?;
    let q = vec![
        rest::p("scope[method_id]", method_id.to_string()),
        rest::p("asset[class]", CLASS),
        rest::p("asset[name]", ticker.to_string()),
        rest::p("limit", limit.to_string()),
    ];
    let path = "/funding/v2/deposit/addresses";
    rest::funding(ctx, Method::GET, path, q, None, Op::Other).await
}

/// Выбор из выданных: не истёкший; без срока — раньше срочных, среди срочных — дальний срок.
/// Возвращает: адрес или None (годных нет). Ошибка — текст (у выбранного tag/memo, нет address).
pub fn pick(v: &Value) -> Result<Option<Entry>, String> {
    let now = Utc::now();
    let rows = v
        .get("addresses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut best: Option<(&Value, Option<DateTime<Utc>>)> = None;
    for r in &rows {
        let exp = num::text_opt(r, "expire_time")
            .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
            .map(|t| t.with_timezone(&Utc));
        if exp.is_some_and(|e| e <= now) {
            continue;
        }
        let better = match (&best, exp) {
            (None, _) => true,
            (Some((_, Some(_))), None) => true,
            (Some((_, Some(b))), Some(e)) => e > *b,
            _ => false,
        };
        if better {
            best = Some((r, exp));
        }
    }
    let Some((r, exp)) = best else {
        return Ok(None);
    };
    crypto(r.get("address_details")).map(|a| Some((a, exp)))
}

/// Адрес из address_details.crypto; tag/memo — ошибка. Ошибка — текст.
pub fn crypto(d: Option<&Value>) -> Result<String, String> {
    let c = d
        .and_then(|d| d.get("crypto"))
        .ok_or("в ответе нет address_details.crypto")?;
    for f in ["tag", "memo"] {
        if let Some(v) = num::text_opt(c, f).filter(|v| !v.trim().is_empty()) {
            return Err(format!("адрес требует {f} ({v}) — не поддерживается"));
        }
    }
    num::text_opt(c, "address").ok_or_else(|| "в ответе нет address".into())
}

/// Ошибка InvalidAddress с площадкой Kraken.
fn invalid(ctx: &Ctx, msg: String) -> GatewayError {
    ctx.tag(GatewayError::new(ErrorCode::InvalidAddress, None, msg))
}
