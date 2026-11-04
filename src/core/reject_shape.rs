//! Файл: core/reject_shape.rs
//! Что делает: решение 155 — «вид причины» для журнала повторов (reject_log.rs): текст без чисел. Числа прибыли и объёма
//! меняются на каждом расчёте, поэтому в сравнение «та же причина или нет» не входят (как в решении 139: причину
//! сравнивают по виду, а не по числам).
//! Кто вызывает: core/candidates.rs, тесты.
//! От чего зависит: ничего.
//! Настройки: нет.

/// Вид причины: каждое число (цифры с «.»/«,» внутри и знаком «-», «−», «+» перед ними) заменено на «#».
/// «прибыль −0.5184 ниже порога 0.1» → «прибыль # ниже порога #». Принимает: текст. Ошибок нет.
pub fn shape(text: &str) -> String {
    let c: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < c.len() {
        let sign = matches!(c[i], '-' | '−' | '+')
            && c.get(i + 1).is_some_and(char::is_ascii_digit)
            && (i == 0 || !c[i - 1].is_alphanumeric());
        if c[i].is_ascii_digit() || sign {
            i += 1;
            while i < c.len()
                && (c[i].is_ascii_digit()
                    || (matches!(c[i], '.' | ',')
                        && c.get(i + 1).is_some_and(char::is_ascii_digit)))
            {
                i += 1;
            }
            out.push('#');
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}
