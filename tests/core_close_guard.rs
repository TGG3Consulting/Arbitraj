//! Файл: tests/core_close_guard.rs
//! Что делает: страж решения 138 по исходникам: возврат USDT после закрытия решается только в ledger::finish
//! (usdt_return_deal::prepare — только в ledger.rs), сам перегон выручки — только из usdt_return_job.rs (решение 163:
//! и повтор — usdt_return_attempt.rs), и никто не
//! ставит Step::Closed в обход ledger::finish. Новый путь закрытия, написанный мимо общего шага, этот тест не пройдёт.
//! Кто вызывает: cargo test.
//! От чего зависит: только исходники src/ (чтение файлов). Моков нет.

use std::path::{Path, PathBuf};

/// Все .rs под папкой. Ошибок нет: нечитаемое пропускается.
fn sources(p: &Path, out: &mut Vec<(PathBuf, String)>) {
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        let path = e.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs")
            && let Ok(s) = std::fs::read_to_string(&path)
        {
            out.push((path, s));
        }
    }
}

#[test]
fn return_step_only_in_ledger_finish() {
    let mut all = vec![];
    sources(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut all,
    );
    let name = |p: &PathBuf| p.file_name().unwrap().to_string_lossy().to_string();
    for (p, s) in &all {
        let f = name(p);
        // Возврат решается только общим шагом закрытия.
        if s.contains("usdt_return_deal::prepare") {
            assert_eq!(
                f, "ledger.rs",
                "{p:?}: возврат USDT — только из ledger::finish"
            );
        }
        // Сам перегон — только из шага закрытия и режима «отдельно».
        if s.contains("usdt_return::relay(") {
            assert_eq!(f, "usdt_return_job.rs", "{p:?}");
        }
        // Решение 163: перегон без извещения о сбое — только шаг закрытия и попытка повтора (повторы, «Перегнать»).
        if s.contains("usdt_return::relay_try(") {
            assert!(
                f == "usdt_return_job.rs" || f == "usdt_return_attempt.rs",
                "{p:?}"
            );
        }
        // Закрыть сделку в обход ledger::finish нельзя: шаг Closed ставит только он.
        for bad in ["step = Step::Closed", "step: Step::Closed"] {
            assert!(
                !s.contains(bad),
                "{p:?}: «{bad}» — закрывайте через ledger::finish"
            );
        }
    }
}
