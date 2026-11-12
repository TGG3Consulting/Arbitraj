//! Файл: core/day_pair.rs
//! Что делает: решение 258 (Тигран 12.11.2026: «Binance DEX и DEX Binance — это тоже разные направления. Их нужно
//! разбить. У каждого свой лимит. И так для всех бирж»; «Каждое направление имеет свой лимит. Все.») — «пара» итога дня
//! и дневного лимита убытка: площадка × тип направления. Правило — то же, что у настроек денег решения 242
//! (core/venue_dir.rs rule; одна функция): биржа → DEX — биржа · cex_dex; DEX → биржа — биржа · dex_cex; биржа → биржа —
//! биржа ПОКУПКИ · cex_cex (Тигран: «лимит распространяется только на биржу в момент покупки, в обратную сторону он не
//! считается»); DEX ↔ DEX — общий набор «dex_dex» (один на все циклы). id пары — как id области настроек VenueDir
//! (settings/venue_dir.rs id: «bingx:cex_dex», «dex_dex»). Площадка без записи в реестре шлюза (вид неизвестен)
//! считается биржей (как раньше в учёте дня).
//! Лимит пары — risk.max_daily_loss_usd: значение пары → значение площадки → вся система (порядок решения 242,
//! Store::decimal_dir). Площадка второго шага — биржа пары; у «DEX ↔ DEX» — площадка DEX реестра (Core::dex_id).
//! Кто вызывает: daily_venue.rs (учёт дня), daily_venue_limit.rs, daily_limit.rs, daily_migrate.rs, daily_load.rs,
//! daily_commit.rs, day_roll.rs, risk.rs, risk_ready.rs, commands.rs, commands_control.rs, notify (меню),
//! report (rows_text.rs, cap_limits.rs).
//! От чего зависит: core/venue_dir.rs (rule), settings/venue_dir.rs (id, разбор), daily_venue.rs (is_dex),
//! notify/human.rs (имя площадки).
//! Настройки: risk.max_daily_loss_usd (max, is_set).

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;
use crate::gateway::errors::GatewayError;
use crate::settings::read::At;
use crate::settings::venue_dir::{DirKind, VenueDir, id, parse_id};

use super::engine::Core;

/// Имя настройки дневного лимита пары.
pub const SETTING: &str = "risk.max_daily_loss_usd";

/// Что делает: пара направления по видам площадок (`dex` — площадка DEX; остальное — биржа). Принимает: направление,
/// признак DEX. Возвращает: id пары. Ошибок нет.
pub fn of_kinds(d: &Direction, dex: impl Fn(&str) -> bool) -> String {
    let k = |v: &str| {
        if dex(v) {
            VenueKind::Dex
        } else {
            VenueKind::Cex
        }
    };
    // rule с известными видами всегда даёт пару (None — только при неизвестном виде, здесь его нет).
    super::venue_dir::rule(d, Some(k(&d.buy_on.0)), Some(k(&d.sell_on.0)))
        .map_or_else(String::new, |vd| id(vd.venue, vd.kind))
}

/// Пара направления по видам площадок реестра шлюза. Принимает: ядро, направление. Ошибок нет.
pub fn of_direction(core: &Core, d: &Direction) -> String {
    of_kinds(d, |v| super::daily_venue::is_dex(core, v))
}

/// Это id пары (а не площадки). Ошибок нет.
pub fn is_pair(p: &str) -> bool {
    parse_id(p).is_some()
}

/// Биржа пары (None — «DEX ↔ DEX» или не пара). Ошибок нет.
pub fn venue_of(p: &str) -> Option<&str> {
    parse_id(p).and_then(|(v, _)| v)
}

/// Тип направления пары. Ошибок нет.
pub fn kind_of(p: &str) -> Option<DirKind> {
    parse_id(p).map(|(_, k)| k)
}

/// Подпись пары для человека: «BingX → DEX», «DEX → BingX», «BingX → биржа», «DEX ↔ DEX». Не пара — имя площадки.
/// Ошибок нет.
pub fn label(p: &str) -> String {
    let n = crate::notify::human::venue;
    match parse_id(p) {
        Some((Some(v), DirKind::CexToDex)) => format!("{} → DEX", n(v)),
        Some((Some(v), DirKind::DexToCex)) => format!("DEX → {}", n(v)),
        Some((Some(v), DirKind::CexCex)) => format!("{} → биржа", n(v)),
        Some((_, DirKind::DexDex)) => "DEX ↔ DEX".into(),
        _ => n(p),
    }
}

/// Пары площадки: у биржи — три (биржа → DEX, DEX → биржа, биржа → биржа), у DEX — «DEX ↔ DEX».
/// Принимает: ядро, id площадки. Ошибок нет.
pub fn of_venue(core: &Core, v: &str) -> Vec<String> {
    if super::daily_venue::is_dex(core, v) {
        return vec![id(v, DirKind::DexDex)];
    }
    [DirKind::CexToDex, DirKind::DexToCex, DirKind::CexCex]
        .into_iter()
        .map(|k| id(v, k))
        .collect()
}

/// Все пары реестра шлюза (биржи — по три, DEX — одна общая «DEX ↔ DEX»). Ошибок нет.
pub fn all(core: &Core) -> Vec<String> {
    let kinds = super::engine::rd(&core.inner.kinds).clone();
    let mut out: Vec<String> = Vec::new();
    for (v, _) in &kinds {
        for p in of_venue(core, &v.0) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Площадка пары: биржа пары; у «DEX ↔ DEX» — DEX реестра (второй шаг чтения лимита, журнал и уведомления).
/// Ошибок нет.
pub fn venue_or_dex(core: &Core, p: &str) -> Option<String> {
    match venue_of(p) {
        Some(v) => Some(v.to_string()),
        None => core.dex_id().map(|v| v.0),
    }
}

/// Что делает: лимит убытка дня пары — значение пары → площадки → вся система (Store::decimal_dir).
/// Принимает: ядро, id пары. Ошибки: NotConfigured — не задан нигде; Internal — неверный вид.
pub fn max(core: &Core, p: &str) -> Result<Decimal, GatewayError> {
    let v = venue_or_dex(core, p);
    let vd = parse_id(p).map(|(venue, kind)| VenueDir {
        venue: venue.unwrap_or(""),
        kind,
    });
    let at = At {
        venue: v.as_deref(),
        coin: None,
    };
    core.inner.deps.settings.decimal_dir(SETTING, vd, at)
}

/// Задан ли лимит пары (в том же порядке, что max). Принимает: ядро, id пары. Ошибок нет.
pub fn is_set(core: &Core, p: &str) -> bool {
    let v = venue_or_dex(core, p);
    let vd = parse_id(p).map(|(venue, kind)| VenueDir {
        venue: venue.unwrap_or(""),
        kind,
    });
    core.inner
        .deps
        .settings
        .resolve_dir(SETTING, vd, v.as_deref(), None)
        .is_some()
}
