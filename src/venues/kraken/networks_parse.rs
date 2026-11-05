//! Файл: venues/kraken/networks_parse.rs
//! Что делает: разбор методов Funding (Beta) Kraken (ввод и вывод) и активов спота в сети монет формата ядра
//! (CoinNetworkInfo): сеть — network_name метода, адрес контракта сети BSC, ввод/вывод, комиссия, минимумы, точность —
//! и строки методов (method_id) для вывода и адреса депозита.
//! Кто вызывает: wallet.rs, tests/kraken_real_data.rs.
//! От чего зависит: num.rs, methods.rs (AssetInfo, Dir), core/types.
//! Настройки: нет (сеть BSC и «минимум депозита неизвестен — 0» передаёт wallet.rs).
//!
//! Официально (https://docs.kraken.com/api-reference/funding-beta/list-funding-methods.md): метод = монета на сети;
//! asset {class, name}, method_id, minimum_amount, maximum_amount, fees {base {asset, amount}, percentage, included, min,
//! max}, network {network_id, network_name, contract_address, on_chain_asset_symbol}, deposit {address_setup_fee,
//! address_generation, shares_addresses_with_method_id, exempted_withdrawal_hold}. Флага «сеть закрыта» у метода нет:
//! метод есть в списке направления = направление открыто (так читается; явно не сказано — проверить живьём), плюс
//! Assets.status монеты. Подтверждений в API нет. Чего ядро не умеет — не угадываем, а закрываем с записью в журнале:
//! процентная комиссия вывода (withdraw_fee = None — ядро не торгует), платный ввод или плата за адрес (ввод закрыт),
//! два метода одной монеты на одной сети (не выбираем — закрыто).

use std::collections::HashMap;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, CoinNetworkInfo, VenueId};
use crate::gateway::errors::GatewayResult;

use super::methods::{AssetInfo, Dir, Method};

/// Строка метода: что нужно для вывода и адреса депозита.
#[derive(Debug, Clone)]
pub struct MethodRow {
    pub ticker: String,
    /// Сеть ядра — network_name метода.
    pub network: String,
    pub dir: Dir,
    pub method_id: String,
}

/// Результат разбора.
#[derive(Default)]
pub struct Parsed {
    pub list: Vec<CoinNetworkInfo>,
    pub rows: Vec<MethodRow>,
    /// Замечания для журнала: «монета/сеть: что закрыто и почему».
    pub notes: Vec<String>,
}

/// Что делает: разбор. Принимает: методы ввода, методы вывода, активы спота, сеть BSC ядра (venue.bsc_network),
/// площадку, «минимум депозита неизвестен — 0». Возвращает: Parsed. Ошибка BadResponse — у метода нет обязательных полей.
pub fn parse(
    dep: &[Value],
    wd: &[Value],
    assets: &HashMap<String, AssetInfo>,
    bsc: &str,
    venue: &VenueId,
    min_ok: bool,
) -> GatewayResult<Parsed> {
    let mut p = Parsed::default();
    let d = group(dep, Dir::Deposit, &mut p)?;
    let w = group(wd, Dir::Withdraw, &mut p)?;
    let mut keys: Vec<&(String, String)> = d.keys().chain(w.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let (ticker, net) = key;
        let (dm, wm) = (d.get(key), w.get(key));
        let st = assets.get(ticker).map(|a| a.status.as_str());
        let (dep_ok, wd_ok) = match st {
            Some("enabled") => (true, true),
            Some("deposit_only") => (true, false),
            Some("withdrawal_only") => (false, true),
            Some(other) => {
                p.notes.push(format!(
                    "{ticker}/{net}: Assets.status «{other}» — ввод и вывод закрыты"
                ));
                (false, false)
            }
            None => {
                p.notes.push(format!(
                    "{ticker}/{net}: монеты нет в Assets — ввод и вывод закрыты"
                ));
                (false, false)
            }
        };
        let mut dep_on = dep_ok && dm.is_some();
        if let Some(m) =
            dm.filter(|m| m.base_fee > Decimal::ZERO || m.pct > Decimal::ZERO || m.paid_setup)
        {
            dep_on = false;
            p.notes.push(format!(
                "{ticker}/{net}: платный ввод или плата за адрес ({}) — ввод закрыт",
                m.id
            ));
        }
        let fee = wm.and_then(|m| (m.pct.is_zero()).then_some(m.base_fee));
        if wm.is_some_and(|m| !m.pct.is_zero()) {
            p.notes.push(format!("{ticker}/{net}: процентная комиссия вывода — ядро её не считает (комиссия неизвестна)"));
        }
        let is_bsc = net == bsc;
        let (cd, cw) = (
            dm.and_then(|m| m.contract.clone()),
            wm.and_then(|m| m.contract.clone()),
        );
        let raw = if cd.is_some() && cw.is_some() && cd != cw {
            p.notes.push(format!(
                "{ticker}/{net}: contract_address ввода и вывода разные — монета не сопоставляется"
            ));
            None
        } else {
            cw.or(cd)
        };
        let contract = raw.filter(|_| is_bsc).and_then(|a| {
            let parsed = a.parse().ok();
            if parsed.is_none() {
                p.notes
                    .push(format!("{ticker}/{net}: contract_address «{a}» не адрес"));
            }
            parsed
        });
        p.list.push(CoinNetworkInfo {
            venue: venue.clone(),
            asset: Asset {
                ticker: ticker.clone(),
                bsc_contract: contract,
            },
            network: net.clone(),
            deposit_enabled: dep_on,
            withdraw_enabled: wd_ok && wm.is_some(),
            withdraw_fee: fee,
            withdraw_min: wm.and_then(|m| m.min),
            withdraw_max: wm.and_then(|m| m.max),
            deposit_min: dm.and_then(|m| m.min).or(min_ok.then_some(Decimal::ZERO)),
            deposit_confirmations: None,
            withdraw_precision: assets.get(ticker).and_then(|a| a.decimals),
            withdraw_step: None,
            updated_at: Utc::now(),
        });
    }
    Ok(p)
}

/// Методы направления по (монета, сеть). Два метода на одну пару — неоднозначно: пара закрыта (в notes).
/// Ошибка BadResponse — нет обязательного поля.
fn group(
    rows: &[Value],
    dir: Dir,
    p: &mut Parsed,
) -> GatewayResult<HashMap<(String, String), Method>> {
    let mut out: HashMap<(String, String), Method> = HashMap::new();
    let mut twice: Vec<(String, String)> = Vec::new();
    for r in rows {
        let Some((ticker, net, m)) = super::methods::parse_method(r)? else {
            continue;
        };
        p.rows.push(MethodRow {
            ticker: ticker.clone(),
            network: net.clone(),
            dir,
            method_id: m.id.clone(),
        });
        let key = (ticker, net);
        if out.insert(key.clone(), m).is_some() {
            twice.push(key);
        }
    }
    for k in twice {
        out.remove(&k);
        p.rows
            .retain(|r| !(r.dir == dir && r.ticker == k.0 && r.network == k.1));
        p.notes.push(format!(
            "{}/{}: два метода {} на одной сети — не выбираем, закрыто",
            k.0,
            k.1,
            dir.path()
        ));
    }
    Ok(out)
}
