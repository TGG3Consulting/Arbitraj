//! Файл: venues/bsc_dex/check/sim_base.rs
//! Что делает: базовый блок eth_simulateV1 (check/simulate.rs) — номер и необязательное время симулированного блока
//! (blockOverrides.time; ревью 122, пункт 5: проба исполнителя — время головы + 1 с вместо +12 с узла BSC).
//! Кто вызывает: check/simulate.rs (run*), exec/executor/sim.rs, exec/dry_build.rs.
//! От чего зависит: ничего.
//! Настройки: нет.

/// Базовый блок симуляции: номер (а не «latest»: срок транзакции считан по времени этого же блока, решение 61) и
/// время симулированного блока. Время не задано — узел ставит сам: у BSC время базового + timestampIncrement = 12 с
/// (bsc 947ba1cb internal/ethapi/simulate.go#L56-L57, #L577-L583), и dex.deadline_s должен быть больше 12 с. Задано
/// (Base::timed — время головы + 1 с, ревью 122) — blockOverrides.time (execution-apis 16caee27
/// src/schemas/execute.yaml#L100-L117: «Time must either increase or remain constant relative to the previous block»;
/// BSC требует строго больше времени базового — simulate.go#L584-L588), срок не надо раздувать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub number: u64,
    pub time: Option<u64>,
}

impl Base {
    /// Блок `number` со временем головы `head_time` (секунды): симулированный блок — на 1 с позже. Ошибок нет.
    pub fn timed(number: u64, head_time: u64) -> Self {
        Base {
            number,
            time: Some(head_time.saturating_add(1)),
        }
    }
}

impl From<u64> for Base {
    fn from(number: u64) -> Self {
        Base { number, time: None }
    }
}
