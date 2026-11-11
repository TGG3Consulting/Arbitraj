//! Файл: report/cap_limits.rs
//! Что делает: решение 256 (дополнение — аудит отчёта, п. 2) — лист «Лимиты на момент выгрузки» отчёта «Сделки, лимиты
//! и капитал»: действующие значения настроек денег и риска так, как их читает сам бот, и откуда взято значение:
//! - money.max_per_trade_usd — по каждой паре площадок реестра (покупка → продажа; DEX → та же DEX — цикл): тот же
//!   вызов, что core/limits_parts.rs parts: Store::decimal_dir(имя, пара core/venue_dir.rs rule, область «площадка
//!   покупки») — значение пары «площадка + тип направления», иначе площадка покупки, иначе вся система;
//! - money.min_profit_usd / money.min_profit_pct — те же пары, core/venue_dir.rs thresholds: пара, иначе вся система;
//! - money.max_per_coin_usd, money.max_in_transit_usd — вся система (limits_parts.rs; бюджет сделки — значение минус
//!   уже вложенное в открытые сделки монеты / в пути на момент решения);
//! - risk.max_daily_loss_usd — по каждой площадке реестра (core/daily_venue_limit.rs: площадка → вся система) и
//!   значение «вся система»; risk.max_daily_loss_total_usd — вся система (core/daily_limit.rs; не задана — общего стопа
//!   по убытку нет).
//!
//! «Откуда взято» — какая область хранилища дала значение (Store::get_exact в порядке чтения бота). Не задано — текст
//! причины бота вместо числа.
//! Кто вызывает: report/cap_send.rs (rows — из ядра), report/cap_xlsx.rs (sheet), tests/report_capital.rs.
//! От чего зависит: settings (Store, Scope, At, venue_dir), core/venue_dir.rs (rule), gateway/contract.rs (VenueKind),
//! notify/human.rs (venue, route), report/results_sheet.rs.
//! Настройки: читает перечисленные выше (только чтение).

use rust_decimal::prelude::ToPrimitive;
use rust_xlsxwriter::{Worksheet, XlsxError};

use crate::core::types::{Asset, Direction, VenueId};
use crate::core::venue_dir::{DirKind, VenueDir, rule};
use crate::gateway::contract::VenueKind;
use crate::notify::human::{route, venue};
use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::settings::store::Store;

use super::results_sheet::{base, header, italic_note, money};

/// Имя листа.
pub const SHEET: &str = "Лимиты на момент выгрузки";

/// Строка листа: настройка, площадка / направление, значение (Err — почему нет), откуда взято, как читает бот.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitRow {
    pub setting: &'static str,
    pub what: String,
    pub value: Result<f64, String>,
    pub source: String,
    pub how: &'static str,
}

/// Тип направления по-русски. Ошибок нет.
fn kind_text(k: DirKind) -> &'static str {
    match k {
        DirKind::CexToDex => "биржа → DEX",
        DirKind::DexToCex => "DEX → биржа",
        DirKind::CexCex => "биржа → биржа",
        DirKind::DexDex => "DEX ↔ DEX",
    }
}

/// Откуда взято значение (порядок чтения бота: пара → площадка → вся система). Ошибок нет.
fn source(s: &Store, name: &str, vd: Option<VenueDir>, at: Option<&str>) -> String {
    if let Some(v) = vd.filter(|v| s.get_exact(name, &v.scope()).is_some()) {
        return match v.kind {
            DirKind::DexDex => "пара «DEX ↔ DEX» (один набор на все циклы)".into(),
            k => format!("пара «{} · {}»", venue(v.venue), kind_text(k)),
        };
    }
    if let Some(a) = at.filter(|a| s.get_exact(name, &Scope::Venue(a.to_string())).is_some()) {
        return format!("площадка {}", venue(a));
    }
    if s.get_exact(name, &Scope::Global).is_some() {
        return "вся система".into();
    }
    "не задано нигде".into()
}

/// Значение так, как читает бот (Store::decimal_dir). Ошибок нет (Err — текст причины).
fn value(s: &Store, name: &str, vd: Option<VenueDir>, at: Option<&str>) -> Result<f64, String> {
    s.decimal_dir(
        name,
        vd,
        At {
            venue: at,
            coin: None,
        },
    )
    .map(|v| v.to_f64().unwrap_or(0.0))
    .map_err(|e| e.message)
}

/// Что делает: строки листа (см. шапку). Принимает: хранилище настроек, площадки реестра с видом. Возвращает: строки.
/// Ошибок нет.
pub fn rows(s: &Store, kinds: &[(VenueId, VenueKind)]) -> Vec<LimitRow> {
    let mut out = Vec::new();
    let mut put = |setting, what: String, vd: Option<VenueDir>, at: Option<&str>, how| {
        let (value, source) = (value(s, setting, vd, at), source(s, setting, vd, at));
        out.push(LimitRow {
            setting,
            what,
            value,
            source,
            how,
        });
    };
    let pairs = kinds.iter().flat_map(|(a, ka)| {
        kinds
            .iter()
            .filter(move |(b, kb)| a != b || (*ka == VenueKind::Dex && *kb == VenueKind::Dex))
            .map(move |(b, kb)| (a, *ka, b, *kb))
    });
    let pairs: Vec<_> = pairs.collect();
    const TRADE: &str =
        "core/limits_parts.rs: пара «площадка + тип» → площадка покупки → вся система";
    const PROFIT: &str = "core/venue_dir.rs thresholds: пара «площадка + тип» → вся система";
    for (setting, how, by_buy) in [
        ("money.max_per_trade_usd", TRADE, true),
        ("money.min_profit_usd", PROFIT, false),
        ("money.min_profit_pct", PROFIT, false),
    ] {
        for (a, ka, b, kb) in &pairs {
            let d = Direction {
                asset: Asset {
                    ticker: String::new(),
                    bsc_contract: None,
                },
                buy_on: (*a).clone(),
                sell_on: (*b).clone(),
            };
            let vd = rule(&d, Some(*ka), Some(*kb));
            let what = format!(
                "{} ({})",
                route(&a.0, &b.0),
                vd.map_or("—", |v| kind_text(v.kind))
            );
            let at = by_buy.then_some(a.0.as_str());
            put(setting, what, vd, at, how);
        }
    }
    let all = "вся система".to_string();
    let lim =
        "core/limits_parts.rs: вся система; бюджет = значение − уже занятое на момент решения";
    put("money.max_per_coin_usd", all.clone(), None, None, lim);
    put("money.max_in_transit_usd", all.clone(), None, None, lim);
    let day = "core/daily_venue_limit.rs: площадка → вся система";
    for (v, _) in kinds {
        put(
            "risk.max_daily_loss_usd",
            venue(&v.0),
            None,
            Some(&v.0),
            day,
        );
    }
    put(
        "risk.max_daily_loss_usd",
        format!("{all} (площадкам без своего значения)"),
        None,
        None,
        day,
    );
    let total = "core/daily_limit.rs: вся система; не задана — общего стопа по убытку нет";
    put("risk.max_daily_loss_total_usd", all, None, None, total);
    out
}

/// Лист «Лимиты на момент выгрузки». Принимает: лист, строки, примечания. Ошибки: XlsxError.
pub fn sheet(ws: &mut Worksheet, rows: &[LimitRow], notes: &[String]) -> Result<(), XlsxError> {
    ws.set_name(SHEET)?;
    let hdr = [
        "Настройка",
        "Площадка / направление",
        "Действующее значение",
        "Откуда взято",
        "Как читает бот",
    ];
    header(ws, &hdr, &[30., 30., 22., 36., 70.])?;
    let (p, m) = (base(), money());
    for (i, r) in rows.iter().enumerate() {
        let row = i as u32 + 1;
        ws.write_string_with_format(row, 0, r.setting, &p)?;
        ws.write_string_with_format(row, 1, &r.what, &p)?;
        match &r.value {
            Ok(v) => ws.write_number_with_format(row, 2, *v, &m)?,
            Err(e) => ws.write_string_with_format(row, 2, e, &p)?,
        };
        ws.write_string_with_format(row, 3, &r.source, &p)?;
        ws.write_string_with_format(row, 4, r.how, &p)?;
    }
    for (k, t) in notes.iter().enumerate() {
        italic_note(ws, rows.len() as u32 + 2 + k as u32, t)?;
    }
    Ok(())
}
