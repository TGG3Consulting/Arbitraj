//! Файл: core/deal_ceiling.rs
//! Что делает: решение 216 — строки расчёта «без потолка» (решение 141) в сообщении Telegram о закрытой сделке
//! (deal.closed). «Потолок» по Тиграну — сколько можно было вложить без ограничения нашего бюджета (opt.invested записи
//! без потолка), а не наш бюджет. Решение 236 (Тигран, 09.11): строки говорят ровно то, что считается, одинаково у
//! плюсовой и минусовой: «🔝 Потолок (макс. прибыль без лимита): opt.invested», «📐 На потолке заработали бы / потеряли
//! бы: opt.profit (расчёт по данным входа)» (не проходит пороги — пометка), «📐 На вложенных: at_budget (тот же расчёт)»
//! — точка модели на объёме, на котором вошли, «💸 Упущено против вложенного: missed». Всё — одна модель по данным
//! момента входа; факт на объёме потолка не считается (продажи на нём не было; пропорция «факт / вложено × потолок»
//! решения 216 убрана — это была не модель). Запись до решения 236 (basis decision_snapshot: σ и курсы позже входа) —
//! строка «⚠️ … несравнимо» перед цифрами. Записи нет — «Без потолка: не
//! посчитано» и причина, если она известна в момент закрытия (выключено, цикл DEX ↔ DEX, ещё считается / ждёт в
//! очереди, сбой чтения базы, решение 217 — сбой расчёта по сделке и «нет снимка рынка» по id сделки). Решение 217:
//! запись без поля basis (посчитана после нашей покупки, до решения 217) — строка «⚠️ … посчитано после покупки —
//! занижено» перед цифрами. Решение 218: запись, пересчитанная по окну сделки (основная — последняя по сделке), —
//! строка «♻️ Без потолка пересчитано по окну сделки…» перед цифрами.
//! Чтение — ОДИН запрос к SQLite (storage/uncapped_db.rs by_deal, последняя запись сделки по её id); очередь расчёта
//! не ждётся — уведомление не задерживается.
//! Кто вызывает: core/ledger.rs (finish: of_closed → deal_text::closed), tests/notify_human.rs.
//! От чего зависит: storage/uncapped_db.rs, uncapped_state.rs (слоты очереди, причины сбоя по сделке), uncapped_types.rs, size_uncapped.rs (At), deal.rs,
//! notify/human.rs (формат чисел, Msg), settings.
//! Настройки: uncapped.enabled (только причина «не посчитано»).

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::notify::human::{self as h, Msg};
use crate::settings::read::At as SAt;
use crate::storage::uncapped_db;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::size_uncapped::{At, Level};
use super::uncapped_state::Key;
use super::uncapped_types::{
    BASIS_RECOMPUTED, BASIS_RECOMPUTED_DEX_PLAN, late_inputs, old_basis, understated,
};

/// Часть записи «без потолка» (core/uncapped_types.rs Record, JSON в SQLite), нужная сообщению.
#[derive(Debug, Clone, Deserialize)]
struct Part {
    opt: At,
    opt_passes: bool,
    missed: Decimal,
    /// Решение 236: у сделки — точка модели на вложенном объёме; у записей без поля — None.
    #[serde(default)]
    at_budget: Option<Level>,
    /// Решение 217: источник данных; записи без поля — «после покупки» (занижено).
    #[serde(default = "old_basis")]
    basis: String,
}

/// Расчёт «без потолка» сделки для сообщения.
#[derive(Debug, Clone, PartialEq)]
pub enum Ceiling {
    /// Запись есть: оптимум без потолка — вложить `invested` → прибыль `profit`; `passes` — проходит ли он пороги
    /// money.min_profit_usd / pct; `missed` — упущено, USDT.
    Known {
        invested: Decimal,
        profit: Decimal,
        passes: bool,
        missed: Decimal,
        /// Решение 236: та же модель на вложенном объёме (Record.at_budget.at); None — точки нет.
        at_invested: Option<At>,
        /// Решение 217: посчитано после нашей покупки (старая запись) — цифры занижены, в сообщении пометка.
        understated: bool,
        /// Решение 236: σ и курсы взяты позже входа (запись до решения 236) — несравнимо, в сообщении пометка.
        late: bool,
        /// Решение 218: запись пересчитана по окну сделки — строка-пометка источника; None — обычная запись.
        recomputed: Option<String>,
    },
    /// Записи нет (или не читается): причина, если известна.
    Unknown(Option<String>),
}

/// Разбор JSON записи. Принимает: тело строки uncapped. Возвращает: Known или Unknown с причиной «запись не читается».
pub fn parse(body: &str) -> Ceiling {
    match serde_json::from_str::<Part>(body) {
        Ok(p) => Ceiling::Known {
            invested: p.opt.invested,
            profit: p.opt.profit,
            passes: p.opt_passes,
            missed: p.missed,
            at_invested: p.at_budget.and_then(|l| l.at),
            understated: understated("deal", &p.basis),
            late: late_inputs("deal", &p.basis),
            recomputed: recomputed_note(&p.basis),
        },
        Err(e) => Ceiling::Unknown(Some(format!("запись не читается: {e}"))),
    }
}

/// Для сообщения о закрытии: у закрытой (шаг Closed) — read, у остановленной и переданной человеку — None (строк нет,
/// чтения нет). Принимает: ядро, сделку. Ошибок нет.
pub async fn of_closed(core: &Core, d: &Deal) -> Option<Ceiling> {
    if d.step != Step::Closed {
        return None;
    }
    Some(read(core, d).await)
}

/// Расчёт «без потолка» сделки — одно чтение последней записи по id сделки, без ожидания очереди.
/// Принимает: ядро, сделку. Возвращает: Ceiling. Ошибок нет — сбой базы становится причиной «не посчитано».
pub async fn read(core: &Core, d: &Deal) -> Ceiling {
    match uncapped_db::by_deal(&core.inner.deps.db, d.id.clone()).await {
        Ok(Some((_, body))) => parse(&body),
        Ok(None) => Ceiling::Unknown(why_none(core, d)),
        Err(e) => Ceiling::Unknown(Some(format!("сбой чтения базы: {e}"))),
    }
}

/// Почему записи нет — только то, что известно наверняка в момент закрытия. Решение 217: сбой расчёта по сделке (и
/// «нет снимка рынка») хранится по id сделки (uncapped_store::deal_failed) — он и есть причина. Принимает: ядро, сделку.
/// Ошибок нет.
fn why_none(core: &Core, d: &Deal) -> Option<String> {
    // Цикл DEX ↔ DEX (buy_on = sell_on, core/cycle_deal.rs): расчёт без потолка ставит только executor.rs.
    if d.direction.buy_on == d.direction.sell_on {
        return Some("для циклов DEX ↔ DEX не считается".into());
    }
    match core
        .inner
        .deps
        .settings
        .flag("uncapped.enabled", SAt::default())
    {
        Ok(true) => {}
        Ok(false) => return Some("расчёт выключен (uncapped.enabled = нет)".into()),
        Err(e) => return Some(e.message),
    }
    {
        let slots = lk(&core.inner.uncapped.slots);
        match slots.get(&Key::Deal(d.id.clone())) {
            Some(s) if s.running => return Some("ещё считается".into()),
            Some(s) if s.pending.is_some() => return Some("ждёт в очереди расчёта".into()),
            _ => {}
        }
    }
    // Решение 217: сбой расчёта по сделке (в том числе «нет снимка рынка») запомнен по id — причина (и убирается).
    lk(&core.inner.uncapped.deal_errors)
        .remove(&d.id)
        .map(|(_, e)| e)
}

/// Строки «без потолка» в сообщение закрытой сделки (см. шапку; одинаковы у плюсовой и минусовой).
/// Принимает: сообщение, расчёт. Возвращает: сообщение со строками. Ошибок нет.
pub fn lines(m: Msg, c: &Ceiling) -> Msg {
    let (invested, profit, passes, missed, at_invested, understated, late, recomputed) = match c {
        Ceiling::Known {
            invested,
            profit,
            passes,
            missed,
            at_invested,
            understated,
            late,
            recomputed,
        } => (
            invested,
            profit,
            passes,
            missed,
            at_invested,
            understated,
            late,
            recomputed,
        ),
        Ceiling::Unknown(why) => {
            let tail = why.as_ref().map(|w| format!(" ({w})")).unwrap_or_default();
            return m.line(format!("Без потолка: не посчитано{tail}"));
        }
    };
    let mut m = m;
    // Решение 217: старая запись (посчитана после нашей покупки) — пометка, цифры занижены.
    if *understated {
        m = m.line("⚠️ Без потолка посчитано после покупки — занижено");
    }
    // Решение 236: запись до решения 236 — σ и курсы позже входа.
    if *late {
        m = m.line("⚠️ Без потолка: σ и курсы взяты позже входа (до решения 236) — несравнимо");
    }
    // Решение 218: пересчёт старой записи по окну сделки — пометка источника.
    if let Some(n) = recomputed {
        m = m.line(n.clone());
    }
    let would = if profit.is_sign_negative() && !profit.is_zero() {
        "потеряли бы"
    } else {
        "заработали бы"
    };
    let mark = if *passes {
        ""
    } else {
        ", не проходит пороги"
    };
    m = m
        .line(format!(
            "🔝 Потолок (макс. прибыль без лимита): {}",
            h::usd(*invested)
        ))
        .line(format!(
            "📐 На потолке {would}: {} (расчёт по данным входа{mark})",
            h::usd(*profit)
        ));
    if let Some(a) = at_invested {
        m = m.line(format!(
            "📐 На вложенных {}: {} (тот же расчёт)",
            h::usd(a.invested),
            h::signed_usd(a.profit)
        ));
    }
    m.line(format!("💸 Упущено против вложенного: {}", h::usd(*missed)))
}

/// Решение 218: пометка записи, пересчитанной по окну сделки (uncapped_types BASIS_RECOMPUTED*); иначе None.
/// Ошибок нет.
fn recomputed_note(basis: &str) -> Option<String> {
    match basis {
        BASIS_RECOMPUTED => Some("♻️ Без потолка пересчитано по окну сделки (рынок до входа)".into()),
        BASIS_RECOMPUTED_DEX_PLAN => Some(
            "♻️ Без потолка пересчитано по окну сделки: биржа — стакан до входа, DEX — из плана (приблизительно)"
                .into(),
        ),
        _ => None,
    }
}
