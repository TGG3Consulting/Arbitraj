//! Файл: venues/bsc_dex/exec/hook_drive.rs
//! Что делает: решение «335 (лесенка влияния)», ревью 2–4 — чистая логика кругов пробы крючков B/C на пути сделки
//! (без узла; цикл с узлом — exec/hook_trade.rs; факты сделки — exec/trade_hooks.rs, здесь реэкспорт; тесты —
//! tests/bsc_dex_hook_drive_logic_1117.rs):
//! - Drive: шаг цикла. Круг 0 — проба на входах оценки. Вход вырос за точки сделки (точный выход «с конца»: влияние
//!   больше оценки → нужно больше входа) — ревью 3, Б3: следующая проба не на новом входе (он снова вырос бы), а на
//!   входе с поднятым влиянием lift = p + (p − e), где p — факт последней пробы, e — влияние, с которым считался её
//!   вход (рост влияния между пробами берётся вдвое); тогда расчёт по факту даёт вход не больше точки пробы — покрыт.
//!   Подъём — только для выбора суммы пробы: итог сделки — по лесенке сделки без подъёма (ближайшая большая точка снята
//!   этой сделкой; с решения 405 влияние между точками — интерполяция, quote/hook_curve.rs at). Не покрыто — отказ этой
//!   сделке с причиной.
//! - Ревью 4 (Н4): предел кругов пробы — НЕ число точек лесенки. Два круга (проба на оценке и проба с подъёмом) —
//!   сам способ, константа алгоритма, а не порог; сверх двух — только hooks.trade_probe_rounds_max (число кругов до
//!   узла на пути сделки; меньше двух — два); не задана — ровно два.
//!
//! Кто вызывает: exec/hook_trade.rs; тесты чистой логики.
//! От чего зависит: exec/trade_hooks.rs (TradeHooks, Want).
//! Настройки: нет своих (предел кругов передаёт exec/hook_trade.rs — hooks.trade_probe_rounds_max).

pub use super::trade_hooks::{Fresh, TradeHooks, Want};

/// Кругов пробы в самом способе (проба на оценке и проба с подъёмом) — константа алгоритма (ревью 4 Н4), не порог.
const METHOD_ROUNDS: u64 = 2;

/// Настройка: предел кругов пробы перед сделкой сверх двух (ревью 4 Н4).
pub const ROUNDS_MAX: &str = "hooks.trade_probe_rounds_max";

/// Что делать дальше.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Все входы покрыты фактом этой сделки — путь готов.
    Done,
    /// Пересчитать пути (с подъёмом или без).
    Eval,
    /// Проба на этих входах.
    Probe(Vec<Want>),
    /// Отказ этой сделке.
    Fail(String),
}

/// Шаг цикла пробы (см. шапку).
#[derive(Debug, Clone)]
pub struct Drive {
    probes: u64,
    limit: u64,
    lifted: bool,
    /// Ревью 4 (Б5): на каком числе проб подъём уже ставился — второй подъём без новой пробы не ставится.
    lift_used: Option<u64>,
}

impl Drive {
    /// Предел кругов пробы: hooks.trade_probe_rounds_max (None — не задана: ровно два), не меньше двух (см. шапку).
    /// Ошибок нет.
    pub fn new(rounds_max: Option<u64>) -> Self {
        Drive {
            probes: 0,
            limit: rounds_max.unwrap_or(METHOD_ROUNDS).max(METHOD_ROUNDS),
            lifted: false,
            lift_used: None,
        }
    }

    /// Независимое ревью t173 (Н-3): поставит ли следующий шаг пробу при непокрытых входах (после расчёта с подъёмом,
    /// первая проба, проба без второго подъёма на том же числе проб) — тогда нужны все входы, в том числе по порядку
    /// исполнения долей (exec/hook_trade.rs). Иначе шаг — пересчёт с подъёмом или отказ, полные входы не нужны.
    /// Ошибок нет.
    pub fn would_probe(&self) -> bool {
        self.lifted
            || self.probes == 0
            || (self.probes < self.limit && self.lift_used == Some(self.probes))
    }

    /// Сколько проб сделано. Ошибок нет.
    pub fn probes(&self) -> u64 {
        self.probes
    }

    /// Что делает: следующий шаг по входам пути после последнего расчёта (см. шапку). Принимает: факты сделки
    /// (подъём ставится и снимается здесь), входы шагов с крючком. Возвращает: Next. Ошибок нет.
    /// Ревью 4 (Б5) — конечность: Done и Fail завершают; Probe увеличивает probes (никогда больше limit: перед каждой
    /// пробой после первой проверено probes < limit); Eval бывает только (а) при постановке подъёма — не больше одного
    /// раза на одно значение probes (lift_used), (б) сразу после расчёта с подъёмом (lifted снимается). Значит, между
    /// двумя ростами probes не больше трёх вызовов, а всего — не больше 3 · (limit + 1) + 1: цикл конечен. Подъём уже
    /// был на этом числе проб, а вход всё равно не покрыт (пул сдвинулся между расчётами) — проба сразу на
    /// непокрытых входах без подъёма.
    pub fn next(&mut self, th: &mut TradeHooks, wants: &[Want]) -> Next {
        if self.lifted {
            self.lifted = false;
            th.lift.clear();
            let need = th.uncovered(wants);
            if need.is_empty() {
                return Next::Eval;
            }
            self.probes += 1;
            return Next::Probe(need);
        }
        let need = th.uncovered(wants);
        if need.is_empty() {
            return Next::Done;
        }
        if self.probes == 0 {
            self.probes = 1;
            return Next::Probe(need);
        }
        if self.probes >= self.limit {
            return Next::Fail(format!(
                "влияние крючка на сумме сделки не покрыто фактом пробы за {} кругов ({ROUNDS_MAX}, не меньше двух) — \
                 вход пула растёт с влиянием",
                self.limit
            ));
        }
        if self.lift_used == Some(self.probes) {
            self.probes += 1;
            return Next::Probe(need);
        }
        self.lift_used = Some(self.probes);
        th.lift = need
            .iter()
            .map(|w| (w.pool.clone(), w.buy, th.lift_of(w)))
            .collect();
        self.lifted = true;
        Next::Eval
    }
}
