//! Файл: core/low_usdt.rs
//! Что делает: алерт «мало USDT» (решение 54): свободных USDT на площадке ≤ alert.min_usdt этой площадки →
//! событие risk.low_usdt в Telegram один раз на пересечение порога; повтор — только после подъёма выше
//! порога и нового падения. Состояние «ниже порога» по площадке хранится в учёте ядра (core_state,
//! вид «low_usdt») — после перезапуска сохраняется, повторного алерта нет. Не задано — алерта нет,
//! в журнал одна запись «не задано» на площадку (до следующего перезапуска).
//! Имя не из группы risk.*: решение 15 не включает торговлю без всех risk.*, а алерт не должен её держать.
//! Автоперегон USDT (usdt_return.rs, решение 34) этим не меняется. Решение 140: у кошелька DEX до первого чтения
//! баланса USDT (строки нет) — «нет данных», а не 0 (free_of): алерта нет; в тексте — «Кошелёк DEX «имя»».
//! Кто вызывает: core/reconcile.rs (каждая сверка балансов), core/engine.rs (load).
//! От чего зависит: engine.rs, persist.rs, settings.
//! Настройки: alert.min_usdt (по площадке).

use rust_decimal::Decimal;

use crate::core::types::{Balance, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::scope::Scope;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Вид записи в учёте ядра.
const KIND: &str = "low_usdt";

/// Свободно котируемой монеты по балансам площадки (решение 140). У кошелька DEX строки котируемой нет — адаптер ещё
/// не знает токен (список токенов приходит с подпиской рынка, сразу после разблокировки его нет): «нет данных», а не 0.
/// У биржи строки нет — биржа так отвечает на нулевой баланс: 0. Принимает: вид площадки, балансы, тикер.
/// Возвращает: None — нет данных. Ошибок нет.
pub fn free_of(kind: VenueKind, b: &[Balance], quote: &str) -> Option<Decimal> {
    let mut rows = b.iter().filter(|x| x.asset == quote).peekable();
    if kind == VenueKind::Dex && rows.peek().is_none() {
        return None;
    }
    Some(rows.map(|x| x.free).sum())
}

/// Восстанавливает состояние «ниже порога» по площадкам при запуске. Ошибка базы — в журнал.
pub async fn load(core: &Core) {
    match super::persist::load::<(VenueId, bool)>(&core.inner.deps.db, KIND).await {
        Ok(list) => lk(&core.inner.usdt_below).extend(list),
        Err(e) => {
            let m = format!("Состояние алерта «мало USDT» не загружено: {e}");
            core.inner
                .deps
                .log(Level::Error, "core.low_usdt", None, m, None)
                .await;
        }
    }
}

/// Проверка одной площадки (у DEX — одного кошелька, решение 132: порог — площадки, состояние — своё у кошелька).
/// Принимает: ядро, площадку, счёт DEX (None — биржа), подпись для текста, свободно USDT, тикер котируемой монеты.
/// Ошибок нет: всё — в журнал и Telegram.
pub async fn check(
    core: &Core,
    venue: &VenueId,
    acc: Option<&crate::core::types::DexAccountId>,
    label: &str,
    free: Decimal,
    quote: &str,
) {
    // Ключ состояния «ниже порога»: площадка или «площадка:кошелёк».
    let key = match acc {
        Some(a) => VenueId(format!("{}:{a}", venue.0)),
        None => venue.clone(),
    };
    let deps = &core.inner.deps;
    // Решение 140: у кошелька DEX — «Кошелёк DEX «имя»», чтобы алерт не путали с биржей.
    let label = match acc {
        Some(a) => {
            let name = deps.settings.accounts().wallet(&a.0).map(|w| w.name);
            format!("Кошелёк DEX «{}»", name.unwrap_or_else(|| a.0.clone()))
        }
        None => label.to_string(),
    };
    let min = match deps
        .settings
        .decimal_in("alert.min_usdt", &Scope::Venue(venue.0.clone()))
    {
        Ok(m) => m,
        Err(e) => {
            if lk(&core.inner.usdt_alert_unset).insert(venue.clone()) {
                deps.log(
                    Level::Warn,
                    "core.low_usdt",
                    Some(&venue.0),
                    format!("Алерт «мало USDT» не работает: {}", e.message),
                    None,
                )
                .await;
            }
            return;
        }
    };
    lk(&core.inner.usdt_alert_unset).remove(venue);
    let below = free <= min;
    let was = lk(&core.inner.usdt_below).insert(key.clone(), below);
    if was == Some(below) {
        return;
    }
    if let Err(e) = super::persist::save(&deps.db, KIND, &key.0, &(key.clone(), below)).await {
        deps.log(
            Level::Error,
            "core.low_usdt",
            Some(&venue.0),
            format!("Состояние алерта не сохранено: {e}"),
            None,
        )
        .await;
    }
    if below {
        let t = format!("{label}: свободно {free} {quote} — не больше порога алерта {min}");
        deps.log(Level::Warn, "core.low_usdt", Some(&venue.0), &t, None)
            .await;
        let tg =
            crate::notify::human::Msg::new(crate::notify::human::WARN, "Мало USDT для покупок")
                .line(format!(
                    "{label}: свободно {} {quote}, порог {}",
                    crate::notify::human::money(free),
                    crate::notify::human::money(min)
                ))
                .todo("пополнить площадку")
                .render();
        deps.notify("risk.low_usdt", Some(&venue.0), &tg).await;
    } else if was.is_some() {
        let t = format!("{label}: свободно {free} {quote} — снова выше порога алерта {min}");
        deps.log(Level::Info, "core.low_usdt", Some(&venue.0), t, None)
            .await;
    }
}
