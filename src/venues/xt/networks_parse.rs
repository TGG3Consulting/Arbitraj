//! Файл: venues/xt/networks_parse.rs
//! Что делает: разбор справочников монет и сетей XT.com в сети формата ядра (CoinNetworkInfo): название сети, адрес
//! контракта у сети BSC, ввод/вывод по сети и по монете, комиссия и минимум вывода, минимум депозита depositMinAmount
//! (t163, решение 366; «0» — минимум 0, решение 370; нет поля — по решению человека); ненулевая комиссия депозита depositFeeRate — ввод по сети
//! не используется.
//! Кто вызывает: wallet.rs (в spawn_blocking), tests/xt_real_data.rs.
//! От чего зависит: num.rs, errors.rs, ctx.rs (тикер), core/types.
//! Настройки: нет (название сети BSC и правило минимума депозита передаёт wallet.rs).
//!
//! Официально:
//! - GET /v4/public/wallet/support/currency (https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetSupportedCurrencies,
//!   `_deposit_withdrawal/supportedCurrenciesGet.md`): [{currency, supportChains[{chain, depositEnabled,
//!   withdrawEnabled, withdrawFeeAmount, withdrawMinAmount, depositFeeRate, contract}]}]; «The currency and chain in
//!   the response need to be used in other deposit/withdrawal API». t163 (решение 366): текущая страница doc.xt.com
//!   описывает ещё depositMinAmount («Minimum deposit amount»), depositFeeRate («Deposit fee rate (percentage)»),
//!   depositConfirmations, withdrawPrecision, withdrawFeeCurrency; в старом зеркале на GitHub (xt-com/xt4-api) их нет.
//!   Живой ответ 17.11.2026: depositMinAmount есть у сетей, сеть BSC — «BNB Smart Chain» (USDT 0.1, USDC 0.01, WIN 0).
//! - GET /v4/public/currencies (https://doc.xt.com/docs/spot/Balance/GetCurrencies, `_balance/currenciesGet.md`): [{currency, depositStatus,
//!   withdrawStatus («0 close 1 open»), maxPrecision}]. Живой ответ 05.11.2026 (решение 184): result — объект
//!   {time, version, currencies[...]}, статусы — числа; разбор принимает и массив, и result.currencies.
//!
//! depositConfirmations читается в deposit_confirmations (ревью t163 Н3; ядро число подтверждений в расчёт не берёт —
//! поле сведений, как у других бирж).
//! Чего в документации НЕТ: смысла depositMinAmount = 0 — решение владельца 17.11 (решение 370): «0» — минимума нет,
//! депозит любой суммы (документация не определяет); смысла withdrawPrecision (число знаков или шаг — не сказано; не читается) — не
//! придумываем.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::deposit_min as dmin;
use crate::gateway::errors::GatewayResult;

use super::ctx::ticker;
use super::{errors, num};

/// Результат разбора.
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    /// Пропущенные или ограниченные сети: «монета/сеть: причина» (для журнала).
    pub skipped: Vec<String>,
}

/// Статусы монеты из /v4/public/currencies: тикер ядра → (ввод открыт, вывод открыт).
/// Ошибка BadResponse — не массив или нет currency.
pub fn parse_statuses(data: &Value) -> GatewayResult<HashMap<String, (bool, bool)>> {
    // Решение 184: живой ответ 05.11.2026 — result объект {time, version, currencies[...]}, а документация показывает
    // массив; принимаются оба вида (tests/data/xt_live_2026-11-05).
    let list = data
        .as_array()
        .or_else(|| data.get("currencies").and_then(Value::as_array))
        .ok_or_else(|| {
            errors::bad(
                "/v4/public/currencies: нет массива (ни result, ни result.currencies)",
                &data.to_string(),
            )
        })?;
    let mut map = HashMap::new();
    for c in list {
        let open = |f: &str| num::text_opt(c, f).as_deref() == Some("1");
        map.insert(
            ticker(&num::text(c, "currency")?),
            (open("depositStatus"), open("withdrawStatus")),
        );
    }
    Ok(map)
}

/// Что делает: разбор справочника сетей. Принимает: ответ support/currency, статусы монет, название сети BSC,
/// площадку, считать ли нулём минимум депозита, когда depositMinAmount = 0 или поля нет (venue.deposit_min_absent_ok).
/// Возвращает: Parsed — сеть с неразборчивым полем исключается, остальные работают.
/// Ошибка BadResponse — ответ не массив или у монеты нет currency.
pub fn parse_networks(
    data: &Value,
    statuses: &HashMap<String, (bool, bool)>,
    bsc: &str,
    venue: &VenueId,
    absent_ok: bool,
) -> GatewayResult<Parsed> {
    let coins = data
        .as_array()
        .ok_or_else(|| errors::bad("support/currency не массив", &data.to_string()))?;
    let mut p = Parsed::default();
    for c in coins {
        let t = ticker(&num::text(c, "currency")?);
        let Some(chains) = c.get("supportChains").and_then(Value::as_array) else {
            continue;
        };
        // Монеты нет в /v4/public/currencies — статус по монете неизвестен: ввод и вывод закрыты.
        let (dep_open, wd_open) = statuses.get(&t).copied().unwrap_or((false, false));
        for n in chains {
            let Some(chain) = num::text_opt(n, "chain") else {
                continue;
            };
            match one(n, &t, &chain, chain == bsc, venue, absent_ok) {
                Ok((mut i, note)) => {
                    i.deposit_enabled &= dep_open;
                    i.withdraw_enabled &= wd_open;
                    if let Some(note) = note {
                        p.skipped.push(format!("{t}/{chain}: {note}"));
                    }
                    p.list.push(i);
                }
                Err(why) => p.skipped.push(format!("{t}/{chain}: {why}")),
            }
        }
    }
    Ok(p)
}

/// Одна сеть монеты. Возвращает: сеть и пояснение ограничения (ввод закрыт из-за depositFeeRate; depositMinAmount не
/// прочитан).
/// Ошибка — текст причины (сеть пропускается).
fn one(
    n: &Value,
    t: &str,
    chain: &str,
    is_bsc: bool,
    venue: &VenueId,
    absent_ok: bool,
) -> Result<(CoinNetworkInfo, Option<String>), String> {
    let e = |e: crate::gateway::errors::GatewayError| e.message;
    let contract = if is_bsc {
        num::text_opt(n, "contract").and_then(|a| a.trim().parse().ok())
    } else {
        None
    };
    let dep_fee = num::dec_opt(n, "depositFeeRate").map_err(e)?;
    let mut deposit = flag(n, "depositEnabled")?;
    // Ревью t163 Н1: нечитаемый depositMinAmount (пустая строка, не число, меньше нуля) не выбрасывает сеть (с ней ушёл
    // бы и вывод с XT, а до решения 366 поле не читалось), причина — в журнал (gateway/deposit_min.rs).
    // Решения 370, 371 (Тигран 17.11; документация смысл «0» не определяет): «0» — минимум 0; нет поля — по
    // venue.deposit_min_absent_ok; нечитаемое (не число, меньше нуля) — неизвестен при любой настройке (ревью Н7).
    // https://doc.xt.com/docs/spot/Deposit&Withdrawal/GetSupportedCurrencies: depositMinAmount «Minimum deposit amount».
    let dm = dmin::classify(num::dec_opt(n, "depositMinAmount").map_err(e));
    let min_note = dmin::note("depositMinAmount", &dm);
    let note = match dep_fee {
        Some(f) if !f.is_zero() && deposit => {
            deposit = false;
            Some(format!(
                "depositFeeRate = {f} — смысл в документации не описан, ввод по сети не используется (вопрос Тиграну)"
            ))
        }
        _ => None,
    };
    let note = match (note, min_note) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    };
    let info = CoinNetworkInfo {
        venue: venue.clone(),
        asset: Asset {
            ticker: t.to_string(),
            bsc_contract: contract,
        },
        network: chain.to_string(),
        deposit_enabled: deposit,
        withdraw_enabled: flag(n, "withdrawEnabled")?,
        withdraw_fee: num::dec_opt(n, "withdrawFeeAmount").map_err(e)?,
        withdraw_fee_pct: None,
        withdraw_min: num::dec_opt(n, "withdrawMinAmount").map_err(e)?,
        withdraw_max: None,
        // t163 (решение 366, решение 370): минимум депозита сети — depositMinAmount, «0» — минимум 0 (решение владельца
        // 17.11); нет поля или не прочитано — по venue.deposit_min_absent_ok, иначе None и ядро сделку с переводом на XT
        // не делает (решение 35).
        deposit_min: dmin::resolve(&dm, absent_ok),
        // Ревью t163 Н3: «Deposit confirmation block count» (doc.xt.com GetSupportedCurrencies); нечитаемое — None.
        deposit_confirmations: num::text_opt(n, "depositConfirmations")
            .and_then(|s| s.parse().ok()),
        withdraw_precision: None,
        withdraw_step: None,
        updated_at: Utc::now(),
    };
    Ok((info, note))
}

/// Флаг сети — boolean. Ошибка — текст (сеть пропускается).
fn flag(n: &Value, field: &str) -> Result<bool, String> {
    match n.get(field) {
        Some(Value::Bool(b)) => Ok(*b),
        other => Err(format!(
            "{field} не boolean ({})",
            other.map_or("нет поля".into(), Value::to_string)
        )),
    }
}
