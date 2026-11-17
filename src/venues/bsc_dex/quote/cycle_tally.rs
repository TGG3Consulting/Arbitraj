//! Файл: venues/bsc_dex/quote/cycle_tally.rs
//! Что делает: решение 404 — итог одного прохода поиска циклов DEX ↔ DEX по базовой монете: сколько циклов просмотрено
//! перебором, сколько с курсом круга > 1 и лучший курс (quote/cycle_rank.rs), сколько отобрано к полному расчёту, и где
//! отобранные отпали — раньше эти места молчали (docs/notes/t189-dexdex-top-cycles-2026-11-18.md, раздел «б»):
//! налоговый или непроверенный токен (cycle_scan tax_free), нет decimals, пул не допущен к котировке (quote/select.rs
//! usable — причина пула), строгий верх ρ < 1 (quote/bound_cycle.rs), вход не подобран (quote/cycle_opt.rs), сигнал не
//! собран (quote/cycle.rs build_signal); и по сигналам — исполнимых, с валовой > 0, с чистой > 0, лучшие валовая и
//! чистая. Текст строки — только числа и постоянные слова (вид причины без чисел не меняется от прохода к проходу —
//! журнал ядра сворачивает повторы в одну строку со счётчиком, core/cycle_scan_log.rs через core/reject_log.rs);
//! примеры причин (тексты с именами пулов) — в данных строки, а не в тексте.
//! Итоги последнего прохода хранятся по площадке (одна запись на проход, замок берётся раз в проход, не на цикл) и
//! отдаются ядру через Venue::dex_cycle_tally (venue.rs).
//! Кто вызывает: quote/cycle_scan.rs (сбор), venue.rs (remember / last).
//! От чего зависит: quote/cycle_rank.rs (Ranked), quote/cycle.rs (CycleSignal), serde_json.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::cycle::CycleSignal;
use super::cycle_rank::Ranked;

/// Почему отобранный цикл не стал сигналом (после отбора лучших).
pub enum Lost {
    /// Небазовый токен цикла не монета подписки, не проверен или с налогом (текст — какой токен).
    Tax(String),
    /// decimals токена не получены (текст ошибки узла).
    Decimals(String),
    /// Пул шага не допущен к котировке (quote/select.rs, текст причины пула).
    Legs(String),
    /// Строгий верх ρ < 1 (решение 373).
    Cut,
    /// Оптимальный вход не подобран (текст ошибки расчёта).
    Opt(String),
    /// Сигнал не собран (текст ошибки).
    Build(String),
}

/// Итог прохода по одной базовой монете.
#[derive(Default)]
pub struct Tally {
    base: String,
    seen: u64,
    above_one: u64,
    best_rate: Option<Decimal>,
    unrated: u64,
    capped: bool,
    picked: u64,
    worst_picked: Option<Decimal>,
    lost: [u64; 6],
    stale: u64,
    examples: [Option<String>; 6],
    signals: u64,
    executable: u64,
    pos_gross: u64,
    pos_net: u64,
    best_gross: Option<Decimal>,
    best_net: Option<Decimal>,
}

impl Tally {
    /// Итог отбора по базовой. Принимает: тикер базовой, итог отбора. Ошибок нет.
    pub fn new(base: &str, r: &Ranked) -> Tally {
        Tally {
            base: base.to_string(),
            seen: r.seen,
            above_one: r.above_one,
            best_rate: r.best,
            unrated: r.unrated,
            capped: r.capped,
            picked: r.paths.len() as u64,
            worst_picked: r.rates.last().copied(),
            ..Default::default()
        }
    }

    /// Отметить потерю отобранного цикла (первый пример причины каждого вида — в данные). Ошибок нет.
    pub fn lost(&mut self, l: Lost) {
        let (i, ex) = match l {
            Lost::Tax(t) => (0, Some(t)),
            Lost::Decimals(t) => (1, Some(t)),
            Lost::Legs(t) => (2, Some(t)),
            Lost::Cut => (3, None),
            Lost::Opt(t) => (4, Some(t)),
            Lost::Build(t) => (5, Some(t)),
        };
        self.lost[i] += 1;
        if self.examples[i].is_none() {
            self.examples[i] = ex;
        }
    }

    /// Отобранные пути, выброшенные гидратацией (пул пропал из реестра, недостоверен, спит — route_cache::hydrate).
    /// Ошибок нет.
    pub fn stale(&mut self, n: u64) {
        self.stale += n;
    }

    /// Учесть сигнал цикла. Ошибок нет.
    pub fn signal(&mut self, s: &CycleSignal) {
        self.signals += 1;
        self.executable += u64::from(s.executable);
        if s.gross_profit > Decimal::ZERO {
            self.pos_gross += 1;
        }
        if self.best_gross.is_none_or(|b| s.gross_profit > b) {
            self.best_gross = Some(s.gross_profit);
        }
        if let Some(n) = s.net_profit {
            if n > Decimal::ZERO {
                self.pos_net += 1;
            }
            if self.best_net.is_none_or(|b| n > b) {
                self.best_net = Some(n);
            }
        }
    }

    /// Строка журнала и данные (примеры причин). Ошибок нет.
    pub fn line(&self) -> (String, Value) {
        let d = |x: Option<Decimal>| {
            x.map_or_else(
                || "нет".to_string(),
                |v| v.round_dp(8).normalize().to_string(),
            )
        };
        let cap = if self.capped {
            ", перебор остановлен пределом dex_dex.rank_max_cycles"
        } else {
            ""
        };
        let text = format!(
            "Поиск циклов DEX ↔ DEX, база {}: просмотрено циклов {}{cap}, курс круга > 1 у {}, лучший курс круга {}, \
             без курса {}; к полному расчёту {} лучших (курс худшего отобранного {}). Отпало: пул пропал из реестра {}, налоговый или \
             непроверенный токен {}, нет decimals {}, пул не допущен {}, строгий верх ρ < 1 {}, вход не подобран {}, \
             сигнал не собран {}. Сигналов {}: исполнимых {}, валовая > 0 у {} (лучшая {}), чистая > 0 у {} (лучшая {}).",
            self.base,
            self.seen,
            self.above_one,
            d(self.best_rate),
            self.unrated,
            self.picked,
            d(self.worst_picked),
            self.stale,
            self.lost[0],
            self.lost[1],
            self.lost[2],
            self.lost[3],
            self.lost[4],
            self.lost[5],
            self.signals,
            self.executable,
            self.pos_gross,
            d(self.best_gross),
            self.pos_net,
            d(self.best_net),
        );
        let data = json!({
            "base": self.base,
            "seen": self.seen,
            "above_one": self.above_one,
            "best_rate": self.best_rate,
            "unrated": self.unrated,
            "capped": self.capped,
            "picked": self.picked,
            "worst_picked_rate": self.worst_picked,
            "lost": {
                "stale": self.stale,
                "tax": self.lost[0], "decimals": self.lost[1], "legs": self.lost[2],
                "cut": self.lost[3], "opt": self.lost[4], "build": self.lost[5],
            },
            "examples": {
                "tax": self.examples[0], "decimals": self.examples[1], "legs": self.examples[2],
                "opt": self.examples[4], "build": self.examples[5],
            },
            "signals": self.signals,
            "executable": self.executable,
            "positive_gross": self.pos_gross,
            "positive_net": self.pos_net,
            "best_gross": self.best_gross,
            "best_net": self.best_net,
        });
        (text, data)
    }
}

/// Строки итога прохода: (текст, данные) на каждую базовую монету.
pub type Lines = Vec<(String, Value)>;

/// Хранилище итогов по площадке.
type Store = Mutex<HashMap<String, Lines>>;

/// Итоги последнего прохода по площадке. Замок НЕ на горячем пути (ревью t189): запись — один раз в конце прохода
/// поиска циклов (cycle_scan.rs scan, в потоке счёта после всех расчётов — помощники quote/par.rs его не берут), чтение —
/// один раз за проход ядра (core/cycle_watch.rs → venue.rs dex_cycle_tally). Ни на цикл, ни на сигнал, ни на ребро он не
/// берётся; держится только на вставку / копию нескольких строк.
static LAST: LazyLock<Store> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Запомнить итоги прохода площадки (раз в проход). Ошибок нет.
pub fn remember(venue: &str, lines: Lines) {
    LAST.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(venue.to_string(), lines);
}

/// Итоги последнего прохода площадки (пусто — проходов не было). Ошибок нет.
pub fn last(venue: &str) -> Lines {
    LAST.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(venue)
        .cloned()
        .unwrap_or_default()
}
