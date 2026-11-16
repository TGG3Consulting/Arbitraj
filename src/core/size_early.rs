//! Файл: core/size_early.rs
//! Что делает: решение 337 (задача t157) — ранний выход подбора объёма (решение 66). Если уже по точкам границ и
//! сетки (и ещё раз — после изломов стакана) лучшая посчитанная прибыль ниже пола «порог прибыли − запас
//! size.early_exit_margin_usd», уточнение пиков не считается, а вызывающий отклоняет кандидата без финальной котировки.
//! Когда прибыльная сделка может потеряться (точное условие, docs/notes/t157-cpu-parallel-2026-11-16.md п. 2б):
//! точка, которую нашли бы изломы или уточнение, выше лучшей посчитанной больше чем на запас и не ниже порога; или
//! финальная котировка (решение 95) лучше своей математики больше чем на запас. Монотонности прибыли от объёма код не
//! гарантирует (умный маршрут, решение 66), поэтому запас — настройка, а не доказательство.
//! Не задана size.early_exit_margin_usd — пола нет, подбор как до 337.
//! Кто вызывает: core/size_search.rs (below), core/profit.rs и core/cycle_plan.rs (floor, reason).
//! От чего зависит: core/size_memo.rs, core/size.rs, settings.
//! Настройки: size.early_exit_margin_usd (вся система; необязательная).

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::settings::store::Store;

use super::size::Curve;
use super::size_memo::Memo;

/// Имя настройки запаса.
pub const MARGIN: &str = "size.early_exit_margin_usd";

/// Что делает: запас раннего выхода. Принимает: настройки. Возвращает: значение; None — не задана или неверна
/// (ранний выход выключен). Ошибок нет.
pub fn margin(s: &Store) -> Option<Decimal> {
    s.decimal(MARGIN, At::default())
        .ok()
        .filter(|m| !m.is_sign_negative())
}

/// Что делает: пол раннего выхода. Принимает: настройки, порог прибыли (USDT). Возвращает: порог − запас; None —
/// запас не задан. Ошибок нет.
pub fn floor(s: &Store, min_usd: Decimal) -> Option<Decimal> {
    margin(s).map(|m| min_usd - m)
}

/// Что делает: лучшая посчитанная точка ниже пола. Принимает: расчёты, пол. Возвращает: true — выходить; нет
/// посчитанных точек или пола — false (обычный ход подбора). Ошибок нет.
pub fn below<C: Curve>(m: &Memo<'_, C>, floor: Option<Decimal>) -> bool {
    match (floor, m.best()) {
        (Some(f), Some(b)) => b.profit < f,
        _ => false,
    }
}

/// Что делает: текст отказа по раннему выходу. Принимает: лучшую прибыль, порог, запас, пояснение выбора.
/// Возвращает: текст. Ошибок нет.
pub fn reason(best: Decimal, min_usd: Decimal, margin: Option<Decimal>, why: &str) -> String {
    format!(
        "ранний выход подбора: лучшая прибыль по границам и сетке {} USDT ниже порога {min_usd} с запасом {MARGIN} {} — \
         уточнение и финальная котировка не считались ({why})",
        best.round_dp(4),
        margin.unwrap_or_default()
    )
}
