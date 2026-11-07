//! Файл: core/start_reason.rs
//! Что делает: решение 147 — точная причина, почему прошедший расчёт кандидат не стал сделкой на входе исполнителя
//! (executor::accepted): направление больше не торгуемо (NotTradable) — что именно у монеты в справочнике в момент
//! старта; по монете уже идёт другое действие (Busy) — какое, если его видно по учёту сделок. Раньше эти отказы были
//! молчаливыми (живой случай: ETH bingx → bsc_dex 04.11.2026 15:08:41 UTC — расчёт «прошёл», сделки нет, причины нигде).
//! Причина уходит в строку «Кандидатов» (поле start) и журнал core.start_refused (candidates.rs).
//! Кто вызывает: core/executor.rs (accepted), тесты.
//! От чего зависит: registry.rs (CoinEntry, Presence, Exclusion), deal.rs, coin_key.rs, core/types.
//! Настройки: нет.

use crate::core::types::{BlockReason, Direction, VenueId};

use super::coin_key::CoinKey;
use super::deal::Deal;
use super::registry::{CoinEntry, Exclusion, Presence};

/// Почему направление не торгуемо в момент старта. Принимает: запись монеты в рабочем справочнике (None — монеты в
/// нём нет), направление. Возвращает: текст причины (первая найденная; порядок — как в CoinEntry::directions). Ошибок нет.
pub fn not_tradable(entry: Option<&CoinEntry>, d: &Direction) -> String {
    let Some(c) = entry else {
        return "монеты нет в рабочем справочнике (ушла из справочника за время расчёта)".into();
    };
    if let Some(b) = &c.block {
        if c.waits_check() {
            let why = c
                .wait
                .as_deref()
                .unwrap_or("причина ожидания от DEX не сообщена");
            return format!("налоги не проверены: {why}");
        }
        return format!("в чёрном списке: {}", block_text(&b.reason));
    }
    if let Some(p) = &c.problem {
        return format!("монету нельзя оценить: {p}");
    }
    let a = side(c, &d.buy_on, "покупки", |p| {
        [
            (p.can_buy, "нельзя купить через API"),
            (p.can_send, "закрыт вывод"),
        ]
    });
    if let Some(why) = a {
        return why;
    }
    let b = side(c, &d.sell_on, "продажи", |p| {
        [
            (p.can_receive, "закрыт ввод (депозит)"),
            (p.can_sell, "нельзя продать через API"),
        ]
    });
    if let Some(why) = b {
        return why;
    }
    if let (Some(why), Some(pa), Some(pb)) = (&c.cex_cex, c.at(&d.buy_on), c.at(&d.sell_on))
        && pa.info.is_some()
        && pb.info.is_some()
    {
        return format!("биржа ↔ биржа не строится: {why}");
    }
    "направления нет среди рабочих направлений монеты (причина в справочнике не найдена)".into()
}

/// Сторона направления: монеты на площадке нет, присутствие исключено (его исключения) или нет нужной роли. Принимает:
/// монету, площадку, «покупки»/«продажи», роли (флаг, текст при false). Возвращает: причину; None — сторона годна.
fn side(
    c: &CoinEntry,
    venue: &VenueId,
    role: &str,
    flags: impl Fn(&Presence) -> [(bool, &'static str); 2],
) -> Option<String> {
    let Some(p) = c.presence.iter().find(|p| &p.venue == venue) else {
        return Some(format!("на площадке {role} {} монеты нет", venue.0));
    };
    if !p.excluded.is_empty() {
        let ex: Vec<String> = p.excluded.iter().map(exclusion_text).collect();
        return Some(format!(
            "на площадке {role} {} монета исключена: {}",
            venue.0,
            ex.join("; ")
        ));
    }
    let off: Vec<&str> = flags(p)
        .iter()
        .filter(|(ok, _)| !ok)
        .map(|(_, t)| *t)
        .collect();
    (!off.is_empty()).then(|| format!("на площадке {role} {}: {}", venue.0, off.join(", ")))
}

/// Текст причины чёрного списка. Ошибок нет.
fn block_text(r: &BlockReason) -> String {
    match r {
        BlockReason::NotChecked => "налоги не проверены".into(),
        BlockReason::CheckFailed { reason } => format!("проверка налогов не прошла: {reason}"),
        BlockReason::TaxAboveLimit { tax, pct, limit } => {
            format!("налог {tax:?} {pct}% выше порога {limit}%")
        }
        BlockReason::Manual { note } => match note {
            Some(n) => format!("ручной чёрный список — {n}"),
            None => "ручной чёрный список (без заметки)".into(),
        },
    }
}

/// Текст исключения присутствия монеты на площадке. Ошибок нет.
fn exclusion_text(e: &Exclusion) -> String {
    match e {
        Exclusion::NoBscNetwork => "нет сети BSC".into(),
        Exclusion::NoContract => "биржа не сообщила адрес контракта".into(),
        Exclusion::NoSpotBook => {
            "стакан не пришёл (пары нет в справочнике пар или подписка не ответила)".into()
        }
        Exclusion::PairNotTrading(s) => format!("пара не торгуется (статус {s})"),
        Exclusion::PairsStale(why) => format!("справочнику пар верить нельзя: {why}"),
        Exclusion::NoApiTrading => "через API нельзя ни купить, ни продать".into(),
        Exclusion::NoUsdtAddress => "не известен адрес USDT в BSC".into(),
        Exclusion::NoUsdtPool => "нет пула с USDT".into(),
        Exclusion::PoolsLoading => "состояние пулов ещё не прочитано".into(),
        Exclusion::PoolsExcluded(r) => format!("все пулы исключены: {}", r.join("; ")),
        Exclusion::IntermediateNotConfigured(why) => {
            format!("промежуточные монеты не настроены: {why}")
        }
        Exclusion::TickerConflict(t) => {
            format!("у адреса на бирже несколько тикеров, выбрать нельзя: {t}")
        }
        Exclusion::NativeVsToken(t) => {
            format!("нативная монета и токен обёртки — разные активы: {t}")
        }
    }
}

/// Почему по монете занято (замок «одна сделка на монету», решение 118). Открытая сделка по монете в учёте — её id,
/// направление и шаг; нет — общая фраза (замок держит предпроверка другого направления до открытия сделки, цикл
/// DEX ↔ DEX или ручное действие «Продать сейчас» / «Вернуть боту»). Принимает: сделки учёта, направление. Ошибок нет.
pub fn busy<'a>(deals: impl Iterator<Item = &'a Deal>, d: &Direction) -> String {
    let key = CoinKey::of(&d.asset);
    let open = deals
        .filter(|x| x.is_open() && CoinKey::of(&x.direction.asset) == key)
        .max_by_key(|x| x.started_at);
    match open {
        Some(x) if x.direction.buy_on == x.direction.sell_on => format!(
            "по монете идёт цикл DEX ↔ DEX: сделка {} (шаг {:?})",
            x.id, x.step
        ),
        Some(x) => format!(
            "по монете идёт другая сделка {} ({} → {}, шаг {:?})",
            x.id, x.direction.buy_on.0, x.direction.sell_on.0, x.step
        ),
        None => "по монете идёт другое действие: предпроверка сделки другого направления, цикл DEX ↔ DEX или ручное \
                 действие («Продать сейчас», «Вернуть боту»)"
            .into(),
    }
}
