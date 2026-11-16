//! Файл: venues/bsc_dex/quote/quote_timing_stages.rs
//! Что делает: этапы и счётчики замера котировки DEX (решение 312) и их имена для записи dex.quote_timing (поле data
//! `<этап>_ms` / `<этап>_n`, текст message). Порядок имён — порядок вариантов.
//! Части вне работы потока счёта (Part) — до работы и после неё в Quoting::priced и быстрый путь quote/fills.rs.
//! Решение 336: части этапа «классика» (в т.ч.) и счётчики допуска пулов и пар через промежуточную.
//! Кто вызывает: quote/quote_timing.rs (накопление), quote/quote_timing_outer.rs (части), quote/quote_timing_log.rs
//! (запись); точки этапов — routing.rs, graph_route.rs, route_cache.rs, route_best.rs, graph_split.rs; частей —
//! quote/mod.rs, quote/fills.rs.
//! От чего зависит: нет.
//! Настройки: нет.

/// Этап котировки. Первые TOP — верхний уровень (не пересекаются; их сумма не больше работы), остальные — части
/// («в т.ч.»): TOP..PATHS_END — этапа Paths, PATHS_END..STAGES — этапа Classic (решение 336; см. PATHS_END).
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
    /// Подготовка глубины на котировку графа (pools/depth_book.rs Book::prep, решение 311 А).
    Prep,
    /// Снимок путей до отбора для повтора графа (graph_fit::snapshot, решение 300; только когда повтор возможен).
    Snapshot,
    /// В т.ч. Paths: пути из кэша (закреплённые или dex.route_paths_ttl_ms) со свежими записями пулов.
    PathsCached,
    /// В т.ч. Paths: граф — снимок фонового графа или построение/кэш (graph_read::graph_for).
    GraphRead,
    /// В т.ч. Paths: перебор путей (route_best / route_paths enumerate).
    Enumerate,
    /// В т.ч. Paths: гидратация путей свежими записями реестра (route_cache::hydrate).
    Hydrate,
    /// Решение 336: в т.ч. Classic — допуск пулов классики (select::usable через usable_reuse.rs).
    ClassicAdmit,
    /// Решение 336: в т.ч. Classic — лучший одиночный пул с USDT (direct::best, с допуском).
    ClassicOne,
    /// Решение 336: в т.ч. Classic — путь через промежуточные монеты (Via::best: допуск P1, P2 и пары).
    ClassicVia,
    /// Решение 336: в т.ч. Classic — второй проход по видимым пулам для сигнала (classic_direct.rs, решение 121).
    ClassicSignal,
    /// Решение 336: в т.ч. Classic — выбор «один пул / через промежуточную» (pick::pick, газ в USDT).
    ClassicPick,
    /// Решение 336: в т.ч. Classic — сборка котировки (costs::quote в direct_route.rs).
    ClassicCosts,
}

/// Этапов всего; первые TOP — верхнего уровня.
pub const STAGES: usize = 23;
pub const TOP: usize = 13;
/// Решение 336: части этапа Paths — TOP..PATHS_END; части этапа Classic — PATHS_END..STAGES (в stages_ms не входят ни
/// те, ни другие: stages_ms — сумма первых TOP). Части классики: ClassicOne, ClassicVia, ClassicPick — по очереди, не
/// пересекаются (сумма ≤ classic); ClassicAdmit и ClassicCosts — внутри них, друг с другом не пересекаются (сумма ≤
/// classic); ClassicSignal — второй проход целиком (его один пул, промежуточная, выбор, допуск, затраты — в своих частях).
pub const PATHS_END: usize = 17;

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
    /// Решение 336: допусков пулов классики (вызовов select::usable).
    AdmitCalls,
    /// Решение 336: из них — из короба подбора (dex.quote_reuse, usable_reuse.rs).
    AdmitHits,
    /// Решение 336: пар пулов через промежуточную (расчётов пути в Via::best).
    ViaPairs,
}

/// Счётчиков всего.
pub const COUNTS: usize = 9;

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
    ("prep", "подготовка глубины"),
    ("snapshot", "снимок путей"),
    ("paths_cached", "из кэша путей"),
    ("graph_read", "граф"),
    ("enumerate", "перебор"),
    ("hydrate", "гидратация"),
    ("classic_admit", "классика: допуск пулов"),
    ("classic_one", "классика: один пул"),
    ("classic_via", "классика: через промежуточную"),
    ("classic_signal", "классика: второй проход (сигнал)"),
    ("classic_pick", "классика: выбор"),
    ("classic_costs", "классика: затраты"),
];

/// Имена счётчиков — в порядке quote_timing::Count.
pub const COUNT_NAMES: [&str; COUNTS] = [
    "paths_in",
    "paths_ok",
    "hops",
    "best_evals",
    "split_steps",
    "split_evals",
    "admit_calls",
    "admit_hits",
    "via_pairs",
];

/// Часть котировки вне работы потока счёта (quote_timing_outer.rs): до работы и после неё в Quoting::priced, у быстрого
/// пути quote/fills.rs — его части.
#[derive(Clone, Copy)]
pub enum Part {
    /// До работы: decimals монеты, USDT и промежуточных (tokens::decimals; у быстрого пути — монеты и USDT).
    Decimals,
    /// До работы: цена газа (gas::wei — запрос к узлу или кэш; send::planned_floor).
    Gas,
    /// После работы: газ разрешений (approve_gas::units).
    Approve,
    /// После работы: газ обёртки BNB (native_gas::cost).
    Native,
    /// После работы: чаевые и газ их перевода (send::planned_tip, send::tip_gas::planned_bnb).
    Tip,
    /// Быстрый путь: лучший одиночный пул (direct::best).
    OnePool,
}

/// Частей всего.
pub const PARTS: usize = 6;

/// Имена частей (поле data `<имя>_ms`, текст message) — в порядке Part.
pub const PART_NAMES: [(&str, &str); PARTS] = [
    ("pre_decimals", "decimals"),
    ("pre_gas", "цена газа"),
    ("post_approve", "газ разрешений"),
    ("post_native", "газ обёртки"),
    ("post_tip", "чаевые"),
    ("fast_pool", "лучший пул"),
];
