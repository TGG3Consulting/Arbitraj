//! Файл: venues/bsc_dex/quote/quote_timing_stages.rs
//! Что делает: этапы и счётчики замера котировки DEX (решение 312) и их имена для записи dex.quote_timing (поле data
//! `<этап>_ms` / `<этап>_n`, текст message). Порядок имён — порядок вариантов.
//! Кто вызывает: quote/quote_timing.rs (накопление), quote/quote_timing_log.rs (запись); точки этапов — routing.rs,
//! graph_route.rs, route_cache.rs, route_best.rs, graph_split.rs.
//! От чего зависит: нет.
//! Настройки: нет.

/// Этап котировки. Первые TOP — верхний уровень (не пересекаются; их сумма не больше работы), остальные — части
/// этапа Paths («в т.ч.»).
#[derive(Clone, Copy)]
pub enum Stage {
    /// Классика (routing.rs: classic_direct / smart_quote).
    Classic,
    /// Набор путей графа (route_cache::paths целиком).
    Paths,
    /// decimals токенов путей (graph_legs::decimals).
    Decimals,
    /// Допуск хопов путей (graph_legs::legs_prep → select::usable).
    Legs,
    /// Газ пути в USDT (route_gas: GasUsdt::new и path; при dex.route_gas_aware = да).
    GasUsdt,
    /// Отбор лучших путей (route_best::best).
    Best,
    /// Сплит (graph_split::split).
    Split,
    /// Исполнимость маршрута (graph_exec::executable).
    Exec,
    /// Сборка котировки (costs::quote).
    Costs,
    /// Повтор графа — лучший исполнимый (graph_fit::best, решение 300).
    Fit,
    /// Выбор котировки-на-сделку (routing.rs: graph_route / route_fit / graph_fit merge, graph_fit::finish).
    Merge,
    /// В т.ч. Paths: пути из кэша (закреплённые или dex.route_paths_ttl_ms) со свежими записями пулов.
    PathsCached,
    /// В т.ч. Paths: граф — снимок фонового графа или построение/кэш (graph_read::graph_for).
    GraphRead,
    /// В т.ч. Paths: перебор путей (route_best / route_paths enumerate).
    Enumerate,
    /// В т.ч. Paths: гидратация путей свежими записями реестра (route_cache::hydrate).
    Hydrate,
}

/// Этапов всего; первые TOP — верхнего уровня.
pub const STAGES: usize = 15;
pub const TOP: usize = 11;

/// Счётчик котировки.
#[derive(Clone, Copy)]
pub enum Count {
    /// Путей на допуск хопов.
    PathsIn,
    /// Путей допущено.
    PathsOk,
    /// Хопов на допуск (сумма по путям).
    Hops,
    /// Расчётов пути в отборе лучших (route_calc::eval; и в повторе графа).
    BestEvals,
    /// Шагов сплита (и в повторе графа).
    SplitSteps,
    /// Расчётов в сплите (route_calc::eval и совместных graph_seq::eval; и в повторе графа).
    SplitEvals,
}

/// Счётчиков всего.
pub const COUNTS: usize = 6;

/// Имена этапов (поле data, текст message) — в порядке quote_timing::Stage.
pub const STAGE_NAMES: [(&str, &str); STAGES] = [
    ("classic", "классика"),
    ("paths", "пути"),
    ("decimals", "decimals"),
    ("legs", "допуск хопов"),
    ("gas_usdt", "газ в USDT"),
    ("best", "отбор лучших"),
    ("split", "сплит"),
    ("exec", "исполнимость"),
    ("costs", "затраты"),
    ("fit", "повтор графа"),
    ("merge", "выбор котировки"),
    ("paths_cached", "из кэша путей"),
    ("graph_read", "граф"),
    ("enumerate", "перебор"),
    ("hydrate", "гидратация"),
];

/// Имена счётчиков — в порядке quote_timing::Count.
pub const COUNT_NAMES: [&str; COUNTS] = [
    "paths_in",
    "paths_ok",
    "hops",
    "best_evals",
    "split_steps",
    "split_evals",
];
