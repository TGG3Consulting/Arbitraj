//! Файл: core/drawdown.rs
//! Что делает: решение 414 (Тигран 19.11.2026, «Да») — лимит потерь направления как ПРОСАДКА ОТ ПИКА теоретического
//! баланса направления (пара «площадка × тип направления» решения 258, core/day_pair.rs) вместо «убыток от итога в
//! момент сброса». Здесь — типы и чистая арифметика (без площадок и без замков):
//! - СЛЕПОК (Snap) — опорный баланс направления: USDT на площадке покупки направления по факту + деньги идущих сделок
//!   ЭТОГО направления, ещё не вернувшиеся на неё, − деньги сделок ДРУГИХ направлений, лежащие на этом счёте (снимает
//!   drawdown_snap.rs; счёт — drawdown_fact.rs; своё и чужое — drawdown_ledger.rs);
//! - ТЕОРЕТИЧЕСКИЙ БАЛАНС = слепок + изменения итога дня пары после слепка (факт закрытых сделок, оценки остановленных
//!   и «Беру на себя» — как итог пары решения 258, решения 53, 152) − плановые затраты возврата USDT её закрытых
//!   сделок, пока возврат не закончен (после — факт уже в итоге, решение 236). Нереализованный результат идущих сделок
//!   НЕ учитывается. Изменения копятся в balance_usd по шагам (advance) — переход через полночь UTC и перезапуск не
//!   теряют их (итог дня пары в новый день начинается с нуля);
//! - ПИК = максимум теоретического баланса с момента слепка (поднимается только когда итог пары изменился — затраты
//!   возврата без изменения итога пик не поднимают); ПРОСАДКА = пик − теоретический баланс;
//! - ПРЕДЕЛ = меньшее из (X % × пик) и (Y USDT): risk.drawdown_pct и risk.drawdown_max_usd (drawdown_cfg.rs); задана
//!   одна — она. Просадка БОЛЬШЕ предела — пауза направления (drawdown_limit.rs).
//!
//! Пример Тиграна: пик 1000, 5 % но не более 30 USDT → предел min(50, 30) = 30; отдали 35 → баланс 965, просадка 35 > 30
//! — пауза; «Сбросить лимит» → новый слепок 965, пик 965.
//! Кто вызывает: drawdown_limit.rs, drawdown_snap.rs, daily.rs (Book), day_view.rs (PairDay), risk_ready.rs, notify.
//! От чего зависит: chrono, rust_decimal, serde.
//! Настройки: нет (пределы — drawdown_cfg.rs).

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Сколько бот ждёт момента, когда у направления нет идущих сделок, чтобы снять «чистый» слепок (дальше — слепок по
/// учёту сделок). Исключение из правила «никаких хардкодов» — решение Тиграна 19.11.2026: «настройка не нужна,
/// константа 2 минуты» (решение 414, CLAUDE.md, раздел «Код»).
pub const CLEAN_WAIT: Duration = Duration::from_secs(120);

/// Как снят слепок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapWay {
    /// Идущих сделок направления не было — USDT на площадке покупки по факту (минус чужое на этом счёте).
    Clean,
    /// За 2 минуты идущие сделки направления не кончились — факт + их деньги в пути − чужое (по учёту сделок).
    Ledger,
}

/// Слепок — опорный баланс направления.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snap {
    /// Слепок, USDT: fact_usd + own_usd − foreign_usd.
    pub base_usd: Decimal,
    /// USDT на площадке покупки по факту баланса (свободно + заморожено; у биржи — торговый счёт и счета «на торговый»).
    pub fact_usd: Decimal,
    /// Деньги идущих сделок этого направления, ещё не вернувшиеся на площадку покупки (по учёту).
    pub own_usd: Decimal,
    /// Деньги сделок других направлений, лежащие на этом счёте (выручка, ждущая возврата на их площадку покупки).
    pub foreign_usd: Decimal,
    pub way: SnapWay,
    /// Когда снят и день UTC слепка.
    pub at: DateTime<Utc>,
    pub day: NaiveDate,
    /// Почему снят: «новый день», «запуск», «Сбросить лимит (интерфейс)» и т.п.
    pub by: String,
    /// Где: площадка покупки словами (у DEX — с кошельком).
    pub place: String,
    /// Сделки «своё» и «чужое» с суммами (для экрана).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub own: Vec<(String, Decimal)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign: Vec<(String, Decimal)>,
}

/// Состояние просадки направления (в книге дня, daily_result — переживает перезапуск и полночь).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawState {
    pub snap: Snap,
    /// Слепок + сумма изменений итога пары после слепка (без плановых затрат возврата), USDT.
    pub balance_usd: Decimal,
    /// Пик теоретического баланса с момента слепка, USDT.
    pub peak_usd: Decimal,
    /// Итог дня пары при прошлом учёте и его день (по ним считается следующее изменение).
    pub seen_usd: Decimal,
    pub seen_day: NaiveDate,
}

/// Запрошен новый слепок (сброс, новый день, запуск): когда, кто; note — почему ещё не снят (последняя причина).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Want {
    pub at: DateTime<Utc>,
    pub by: String,
    /// Запросил человек («Сбросить лимит»): снятый слепок уходит в Telegram (risk.daily_limit_reset).
    #[serde(default)]
    pub human: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Пределы направления из настроек (drawdown_cfg.rs): X % и Y USDT; хотя бы одно задано.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    pub pct: Option<Decimal>,
    pub usd: Option<Decimal>,
}

/// Просадка направления для экрана, меню и проверки.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DrawView {
    pub snap: Snap,
    /// Пик (не ниже теоретического баланса), теоретический баланс, плановые затраты возврата в нём, просадка, USDT.
    pub peak_usd: Decimal,
    pub balance_usd: Decimal,
    pub pending_cost_usd: Decimal,
    pub drawdown_usd: Decimal,
    /// risk.drawdown_pct и X % × пик в USDT; risk.drawdown_max_usd. None — не задано.
    pub pct: Option<Decimal>,
    pub pct_usd: Option<Decimal>,
    pub usd: Option<Decimal>,
    /// Предел — меньшее из заданных; какой сработал: «pct» или «usd».
    pub limit_usd: Decimal,
    pub by: String,
    /// Просадка больше предела.
    pub exceeded: bool,
}

/// Что делает: состояние сразу после слепка: баланс = слепок, пик = слепок − плановые затраты возврата (теоретический
/// баланс в момент слепка). Принимает: слепок, итог дня пары сейчас и день, плановые затраты возврата. Ошибок нет.
pub fn fresh(snap: Snap, seen: Decimal, day: NaiveDate, pending: Decimal) -> DrawState {
    DrawState {
        balance_usd: snap.base_usd,
        peak_usd: snap.base_usd - pending,
        seen_usd: seen,
        seen_day: day,
        snap,
    }
}

/// Что делает: учёт нового итога дня пары: баланс += изменение (в тот же день — разница с прошлым, в новый день —
/// весь итог нового дня: прошлый день учтён целиком при прошлых пересчётах); итог изменился — пик = max(пик,
/// теоретический баланс). Принимает: состояние, итог дня пары, день, плановые затраты возврата. Ошибок нет.
pub fn advance(st: &mut DrawState, result: Decimal, day: NaiveDate, pending: Decimal) {
    // Ревью t199 (мелкое 1): пересчёт за прошедший день, пришедший после полуночи, — не учитывается (итог того дня уже
    // учтён, а разница с итогом нового дня была бы неверной).
    if day < st.seen_day {
        return;
    }
    let delta = if st.seen_day == day {
        result - st.seen_usd
    } else {
        result
    };
    st.balance_usd += delta;
    st.seen_usd = result;
    st.seen_day = day;
    if !delta.is_zero() {
        st.peak_usd = st.peak_usd.max(st.balance_usd - pending);
    }
}

/// Что делает: просадка и предел. Предел — меньшее из X % × пик (пик ≤ 0 — 0) и Y USDT; задано одно — оно.
/// Принимает: состояние, плановые затраты возврата, пределы. Возвращает: вид для экрана. Ошибок нет.
pub fn view(st: &DrawState, pending: Decimal, l: Limit) -> DrawView {
    let bal = st.balance_usd - pending;
    let peak = st.peak_usd.max(bal);
    let pct_usd = l
        .pct
        .map(|p| (p / Decimal::ONE_HUNDRED * peak.max(Decimal::ZERO)).round_dp(8));
    let (limit, by) = match (pct_usd, l.usd) {
        (Some(a), Some(b)) if b < a => (b, "usd"),
        (Some(a), _) => (a, "pct"),
        (None, Some(b)) => (b, "usd"),
        (None, None) => (Decimal::ZERO, ""),
    };
    let draw = (peak - bal).max(Decimal::ZERO);
    DrawView {
        snap: st.snap.clone(),
        peak_usd: peak,
        balance_usd: bal,
        pending_cost_usd: pending,
        drawdown_usd: draw,
        pct: l.pct,
        pct_usd,
        usd: l.usd,
        limit_usd: limit,
        by: by.to_string(),
        exceeded: !by.is_empty() && draw > limit,
    }
}
