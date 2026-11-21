//! Файл: settings/venue_dir.rs
//! Что делает: решение 242 — область «площадка + тип направления» (Scope::VenueDir) и чтение настройки с откатом:
//! значение пары «площадка · тип направления», а если оно не задано — прежняя цепочка (монета → площадка → вся система,
//! Store::resolve). Пустая новая область — чтение побайтно прежнее (тот же Store::decimal / resolve).
//! Типы направлений: биржа → DEX, DEX → биржа, биржа → биржа (у биржи) и DEX ↔ DEX (у DEX). Какая площадка и какой тип у
//! сделки (правило стороны) — решает ядро (core/venue_dir.rs); здесь только хранение и чтение.
//! id области — «площадка:тип» («bingx:cex_dex»), ключ в SQLite — «venue_dir:bingx:cex_dex» (settings/scope.rs).
//! «DEX ↔ DEX» (Тигран 09.11) — ОДИН набор значений на все сделки DEX ↔ DEX, не по DEX и не по кошельку: id «dex_dex»
//! без площадки (ключ «venue_dir:dex_dex»).
//! Дополнение п. 2 (Тигран 09.11: согласовано): у profit.price_move_k значение монеты сильнее пары —
//! decimal_coin_dir (монета → пара → вся система); у остальных четырёх — как было (пара сильнее).
//! Кто вызывает: settings/scope.rs (разбор id), settings/value.rs (проверка), core/venue_dir.rs и места чтения
//! money.min_profit_usd, money.min_profit_pct, money.max_per_trade_usd, profit.price_move_k, profit.closure_share;
//! решение 260 — signal.probe_usd (core/signal_probe.rs, та же область, что у money.max_per_trade_usd).
//! От чего зависит: settings/store.rs, settings/read.rs (At), gateway/errors.rs, serde.
//! Настройки: читает те, у которых в каталоге есть область VenueDir (SettingDef::dir_kinds).

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::read::At;
use super::scope::Scope;
use super::store::Store;

/// Тип направления площадки (решение 242). Имена в JSON и в id области — как key().
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DirKind {
    /// Биржа → DEX: покупка на бирже, продажа на DEX (значение — у биржи).
    #[serde(rename = "cex_dex")]
    CexToDex,
    /// DEX → биржа: покупка на DEX, продажа на бирже (значение — у биржи).
    #[serde(rename = "dex_cex")]
    DexToCex,
    /// Биржа → биржа (CEX ↔ CEX, решение 107): значение — у биржи покупки.
    #[serde(rename = "cex_cex")]
    CexCex,
    /// DEX ↔ DEX (цикл, решение 85): один набор значений на все сделки DEX ↔ DEX (id без площадки).
    #[serde(rename = "dex_dex")]
    DexDex,
}

impl DirKind {
    /// Все типы по порядку экрана. Ошибок нет.
    pub const ALL: [DirKind; 4] = [
        DirKind::CexToDex,
        DirKind::DexToCex,
        DirKind::CexCex,
        DirKind::DexDex,
    ];

    /// Ключ типа в id области и в JSON. Ошибок нет.
    pub fn key(self) -> &'static str {
        match self {
            DirKind::CexToDex => "cex_dex",
            DirKind::DexToCex => "dex_cex",
            DirKind::CexCex => "cex_cex",
            DirKind::DexDex => "dex_dex",
        }
    }

    /// Разбор ключа. Принимает: ключ. Возвращает: тип или None — неизвестный ключ.
    pub fn parse(key: &str) -> Option<DirKind> {
        DirKind::ALL.into_iter().find(|k| k.key() == key)
    }
}

/// Площадка и тип направления, у которых берётся значение сделки (правило стороны — core/venue_dir.rs).
/// У DexDex площадка в id области не входит (один набор на все DEX ↔ DEX).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueDir<'a> {
    pub venue: &'a str,
    pub kind: DirKind,
}

impl VenueDir<'_> {
    /// Область хранилища для этой пары. Ошибок нет.
    pub fn scope(&self) -> Scope {
        Scope::VenueDir(id(self.venue, self.kind))
    }
}

/// id области: «площадка:тип»; у DEX ↔ DEX — «dex_dex» без площадки. Принимает: площадку, тип. Ошибок нет.
pub fn id(venue: &str, kind: DirKind) -> String {
    match kind {
        DirKind::DexDex => kind.key().to_string(),
        _ => format!("{venue}:{}", kind.key()),
    }
}

/// Разбор id области: «площадка:тип» (тип — после последнего «:», не dex_dex) или «dex_dex».
/// Возвращает: (площадка — None у dex_dex, тип) или None — площадка пустая, тип неизвестен, «площадка:dex_dex».
pub fn parse_id(id: &str) -> Option<(Option<&str>, DirKind)> {
    if id == DirKind::DexDex.key() {
        return Some((None, DirKind::DexDex));
    }
    let (venue, kind) = id.rsplit_once(':')?;
    let kind = DirKind::parse(kind).filter(|k| *k != DirKind::DexDex)?;
    if venue.trim().is_empty() {
        return None;
    }
    Some((Some(venue), kind))
}

impl Store {
    /// Значение ровно в области «площадка + тип направления»; None — пары нет или значение не задано.
    fn venue_dir_exact(&self, name: &str, vd: Option<VenueDir>) -> Option<Value> {
        vd.and_then(|vd| self.get_exact(name, &vd.scope()))
    }

    /// Действующее значение с областью «площадка + тип направления» (решение 242): значение пары, иначе прежняя
    /// цепочка Store::resolve (монета → площадка → вся система). Принимает: имя, пару (None — нет), площадку и монету.
    /// Возвращает: значение или None — не задано нигде.
    pub fn resolve_dir(
        &self,
        name: &str,
        vd: Option<VenueDir>,
        venue: Option<&str>,
        coin: Option<&str>,
    ) -> Option<Value> {
        self.venue_dir_exact(name, vd)
            .or_else(|| self.resolve(name, venue, coin))
    }

    /// Десятичное число с областью «площадка + тип направления» (решение 242): значение пары, иначе ровно прежнее
    /// чтение Store::decimal(name, at) (те же значение и ошибки). Принимает: имя, пару (None — нет), прежнюю область.
    /// Ошибки: NotConfigured — не задано нигде; Internal — хранится в неверном виде.
    pub fn decimal_dir(
        &self,
        name: &str,
        vd: Option<VenueDir>,
        at: At,
    ) -> Result<Decimal, GatewayError> {
        match self.venue_dir_exact(name, vd) {
            Some(v) => v
                .as_str()
                .and_then(|s| Decimal::from_str(s).ok())
                .ok_or_else(|| {
                    GatewayError::new(
                        ErrorCode::Internal,
                        None,
                        format!("Настройка {name} хранится в неверном виде"),
                    )
                }),
            None => self.decimal(name, at),
        }
    }

    /// Что делает: решение 242, дополнение п. 2 (Тигран 09.11: согласовано) — десятичное число, у которого значение
    /// МОНЕТЫ сильнее пары: монета → площадка + тип направления → прежняя цепочка без монеты (вся система). Только для
    /// profit.price_move_k (остальные настройки решения 242 — decimal_dir: пара сильнее). Значение монеты — ровно в
    /// области «монета» по id монеты (как первый шаг Store::resolve; старые значения по тикеру к этой настройке и раньше
    /// не применялись — coin_alias.rs). Принимает: имя, пару (None — нет), область чтения (coin — id монеты).
    /// Ошибки: как decimal_dir — NotConfigured (не задано нигде), Internal (неверный вид).
    pub fn decimal_coin_dir(
        &self,
        name: &str,
        vd: Option<VenueDir>,
        at: At,
    ) -> Result<Decimal, GatewayError> {
        let coin_set = at
            .coin
            .is_some_and(|c| self.get_exact(name, &Scope::Coin(c.to_string())).is_some());
        if coin_set {
            return self.decimal(name, at);
        }
        self.decimal_dir(name, vd, at)
    }
}
