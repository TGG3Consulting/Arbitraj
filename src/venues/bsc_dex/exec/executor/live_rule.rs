//! Файл: venues/bsc_dex/exec/executor/live_rule.rs
//! Что делает: решение 400 (t185) — правило приёмки исполнителя v3 ПО КЛАССАМ шага из записи симуляции на живых пулах
//! сети (tests/data/executor_v3_live_<дата>/: README.md, summary.json, hooks_reject.json; процедура —
//! docs/notes/t181-executor-all-kinds-2026-11-17.md, п. 5). Чистая функция judge: тексты записи → вердикт по каждому
//! классу (вид шага 19 / 20 / 21 × без крючка / с крючком, + Infinity Stable). Список принятых классов в коде не пишется —
//! он выводится из записи этим правилом.
//! Правило (строго, без допусков; решение 400):
//! - запись относится к коду, только если в её README есть SHA-256 текущего bin-runtime;
//! - пул класса доказан, если ОБА одиночных вызова — (а) покупка «USDT → монета» и (б) продажа того же пула — прошли
//!   (status 1), получено = кредит события Swap менеджера (got_eq_swap_credit), получено = квотер DEX В ТОМ ЖЕ
//!   состоянии ((а) — quoter_out на H: (а) — первый вызов поверх H; (б) — quoter_out_after_a: квотер в той же
//!   симуляции сразу после (а); quoter_out (б) «на H» считает до покупки (а) — другое состояние, не сравнивается) и
//!   x_left = 0;
//! - откат без записи в hooks_reject.json (тот же пул и case), остаток на исполнителе, получено ≠ кредит Swap или ≠
//!   квотер в том же состоянии — «необъяснённое»: класс не принимается;
//! - класс принят: необъяснённого нет и доказано не меньше MIN_POOLS пулов; класс «с крючком» (и Infinity Stable) не
//!   принимается: бот не отсекает пул, чей крючок отвергает исполнитель (пробы шага через исполнитель у пулов V4 /
//!   Infinity нет — их проба идёт Universal Router, check/probe.rs part; docs/TASKS.md t185).
//! Кто вызывает: exec/executor/live_proof.rs, тесты.
//! От чего зависит: serde_json, exec/executor/live_row.rs (разбор строки summary.json).
//! Настройки: нет (правило приёмки собранного кода, решение 400; не настройка торговли).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::live_row::{Row, row, text};

/// Решение 400: наименьшее число доказанных пулов класса — нижняя граница процедуры симуляции («по 2–3 пула каждого
/// вида», заметка t181, п. 5). Правило приёмки, не настройка бота.
pub const MIN_POOLS: usize = 2;

/// Крючок пула класса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hook {
    /// hooks ключа = 0.
    Without,
    /// Любой ненулевой крючок (кроме пула Infinity Stable).
    With,
    /// Infinity Stable — пул в крючке пула CL (шаг INF_CL); в summary.json — строка с полем `"stable": true`.
    Stable,
}

/// Класс шага: номер вида шага контракта (kind.rs: UNI_V4 = 19, INF_CL = 20, INF_BIN = 21) и крючок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Class {
    pub kind: u8,
    pub hook: Hook,
}

/// Все классы шагов пулов менеджеров исполнителя v3 (виды шага контракта 19–21; Infinity Stable — только вид 20).
pub const CLASSES: [Class; 7] = [
    Class {
        kind: 19,
        hook: Hook::Without,
    },
    Class {
        kind: 19,
        hook: Hook::With,
    },
    Class {
        kind: 20,
        hook: Hook::Without,
    },
    Class {
        kind: 20,
        hook: Hook::With,
    },
    Class {
        kind: 20,
        hook: Hook::Stable,
    },
    Class {
        kind: 21,
        hook: Hook::Without,
    },
    Class {
        kind: 21,
        hook: Hook::With,
    },
];

impl Class {
    /// Имя класса для интерфейса и причин. Ошибок нет.
    pub fn label(self) -> String {
        let name = match self.kind {
            19 => "Uniswap V4",
            20 => "Infinity CL",
            21 => "Infinity Bin",
            _ => "вид ?",
        };
        match self.hook {
            Hook::Without => format!("{name} (вид {}) без крючка", self.kind),
            Hook::With => format!("{name} (вид {}) с крючком", self.kind),
            Hook::Stable => format!("Infinity Stable (вид {})", self.kind),
        }
    }
}

/// Вердикт по классу.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub class: Class,
    /// Пулов класса в одиночных вызовах (а) / (б) записи.
    pub pools: usize,
    /// Из них доказано по правилу.
    pub proven: usize,
    pub accepted: bool,
    /// Почему принят / не принят.
    pub why: String,
}

/// Учёт одного класса.
#[derive(Default)]
struct Tally {
    pools: BTreeSet<String>,
    buy: BTreeMap<String, bool>,
    sell: BTreeMap<String, bool>,
    bad: Vec<String>,
    empty: bool,
}

/// Вердикты по всем классам (порядок CLASSES). Принимает: README.md, summary.json, hooks_reject.json записи и SHA-256
/// текущего bin-runtime. Ошибки (текст): SHA не записан, файлы не JSON, строка не по формату (неизвестный case, нет
/// поля, статус не 0/1, число не десятичной строкой).
pub fn judge(
    readme: &str,
    summary: &str,
    reject: &str,
    current: Option<&str>,
) -> Result<Vec<Verdict>, String> {
    let cur = current.ok_or("SHA-256 текущего bin-runtime не записан (ArbExecutor.sha256)")?;
    let bound = readme
        .to_ascii_lowercase()
        .contains(&cur.to_ascii_lowercase());
    let rows: Vec<Value> =
        serde_json::from_str(summary).map_err(|e| format!("summary.json: {e}"))?;
    let rej: Vec<Value> =
        serde_json::from_str(reject).map_err(|e| format!("hooks_reject.json: {e}"))?;
    let mut explained = BTreeSet::new();
    for r in &rej {
        explained.insert((text(r, "pool_id")?.to_ascii_lowercase(), text(r, "case")?));
    }
    let mut t: BTreeMap<Class, Tally> = BTreeMap::new();
    for v in &rows {
        if text(v, "case")? == "нет пулов" {
            for c in CLASSES
                .iter()
                .filter(|c| Some(u64::from(c.kind)) == v["kind"].as_u64())
            {
                t.entry(*c).or_default().empty = true;
            }
            continue;
        }
        let r = row(v)?;
        tally(t.entry(r.class).or_default(), &r, &explained);
    }
    Ok(CLASSES
        .iter()
        .map(|c| verdict(*c, t.get(c), bound, cur))
        .collect())
}

/// Учёт строки в классе. Ошибок нет.
fn tally(t: &mut Tally, r: &Row, explained: &BTreeSet<(String, String)>) {
    let at = format!("({}) пул {}", r.case, r.pool);
    let single = r.case == "a" || r.case == "b";
    if single {
        t.pools.insert(r.pool.clone());
    }
    if !r.x_zero {
        t.bad.push(format!("{at}: на исполнителе остались токены"));
    }
    let proof = if !r.ok {
        if !explained.contains(&(r.pool.clone(), r.case.clone())) {
            t.bad
                .push(format!("{at}: откат вне hooks_reject.json — {}", r.revert));
        }
        false
    } else if !single {
        // (в) сплит и (г) цикл — дополнительные вызовы: от них нужны только успех и пустой исполнитель.
        true
    } else if !r.credit {
        t.bad.push(format!("{at}: получено ≠ кредит события Swap"));
        false
    } else {
        match (r.got, r.quoter) {
            (Some(g), Some(q)) if g == q => true,
            (Some(_), Some(_)) => {
                t.bad
                    .push(format!("{at}: получено ≠ квотер в том же состоянии"));
                false
            }
            // Квотер не ответил — сравнить не с чем: пул не доказан (не «необъяснённое»).
            _ => false,
        }
    };
    let side = match r.case.as_str() {
        "a" => &mut t.buy,
        "b" => &mut t.sell,
        _ => return,
    };
    *side.entry(r.pool.clone()).or_insert(true) &= proof;
}

/// Вердикт класса. Ошибок нет.
fn verdict(c: Class, t: Option<&Tally>, bound: bool, cur: &str) -> Verdict {
    let empty = Tally::default();
    let t = t.unwrap_or(&empty);
    let proven = t
        .pools
        .iter()
        .filter(|p| t.buy.get(*p) == Some(&true) && t.sell.get(*p) == Some(&true))
        .count();
    let mut why = Vec::new();
    if !bound {
        why.push(format!(
            "в README записи нет SHA-256 текущего bin-runtime {cur} — запись о другом коде"
        ));
    }
    if c.hook != Hook::Without {
        why.push(
            "класс с крючком: бот не отсекает пул, чей крючок отвергает исполнитель (проба шага через исполнитель у \
             пулов V4 / Infinity не делается) — не принимается"
                .into(),
        );
    }
    if t.empty && t.pools.is_empty() {
        why.push("в записи «нет пулов» — не проверен".into());
    }
    if proven < MIN_POOLS {
        why.push(format!(
            "доказано пулов {proven} из {} (нужно не меньше {MIN_POOLS})",
            t.pools.len()
        ));
    }
    if !t.bad.is_empty() {
        why.push(format!("необъяснённое: {}", t.bad.join("; ")));
    }
    let accepted = why.is_empty();
    let why = if accepted {
        format!(
            "принят: доказано {proven} из {} пулов — (а) и (б) прошли, получено = кредит Swap = квотер в том же \
             состоянии, на исполнителе 0",
            t.pools.len()
        )
    } else {
        why.join("; ")
    };
    Verdict {
        class: c,
        pools: t.pools.len(),
        proven,
        accepted,
        why,
    }
}
