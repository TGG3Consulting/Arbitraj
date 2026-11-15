//! Файл: tests/golden_common/mod.rs
//! Что делает: эталонные снимки поведения парных пулов (решение 295, шаг 0). Снимок — текст, записанный КОДОМ ДО
//! правки многомонетных пулов (ветка t126 от cab52fda) на записанных данных; после каждого шага правки тот же тест
//! обязан дать тот же текст байт в байт. Запись снимка — только по явной переменной окружения ARB_GOLDEN_WRITE=1
//! (один раз, на неизменённом коде; файл коммитится первым коммитом решения 295).
//! Кто вызывает: tests/bsc_dex_multi_pair_golden_*.rs.
//! От чего зависит: std; файлы tests/data/multi_pair_2026-11-15/<имя>.txt.

/// Что делает: сверяет текст со снимком `name` (или записывает снимок при ARB_GOLDEN_WRITE=1).
/// Принимает: имя снимка (без расширения), текст. Возвращает: ничего.
/// Ошибки: паника теста — нет файла снимка или текст отличается (печатается первая отличающаяся строка).
pub fn check(name: &str, text: &str) {
    let path = format!(
        "{}/tests/data/multi_pair_2026-11-15/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var("ARB_GOLDEN_WRITE").as_deref() == Ok("1") {
        std::fs::write(&path, text).expect("запись снимка");
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("нет снимка {path}"));
    if want != text {
        let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), text.lines().collect());
        let at = w.iter().zip(&g).position(|(a, b)| a != b).unwrap_or(w.len().min(g.len()));
        panic!(
            "снимок {name} изменился: строк было {}, стало {}; первая разница — строка {}:\n было: {:?}\n стало: {:?}",
            w.len(),
            g.len(),
            at + 1,
            w.get(at),
            g.get(at)
        );
    }
}

/// Что делает: строки в устойчивом порядке (сортировка) одним текстом — для коллекций с порядком HashMap.
/// Принимает: строки. Возвращает: текст, строки через перевод строки, с переводом в конце. Ошибок нет.
pub fn sorted(mut lines: Vec<String>) -> String {
    lines.sort();
    let mut s = lines.join("\n");
    s.push('\n');
    s
}
