//! Файл: venues/bsc_dex/exec/executor/live_class.rs
//! Что делает: решение 400 (t185) — класс шага исполнителя v3 для приёмки на живых пулах: номер вида шага контракта
//! (kind.rs: UNI_V4 = 19, INF_CL = 20, INF_BIN = 21) и крючок пула (без крючка, с крючком, Infinity Stable); все классы
//! и их имена для интерфейса (вынесено из live_rule.rs: файл был больше 200 строк).
//! Кто вызывает: exec/executor/{live_rule, live_row, live_proof}.rs, тесты (через реэкспорт live_rule).
//! От чего зависит: нет.
//! Настройки: нет.

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
