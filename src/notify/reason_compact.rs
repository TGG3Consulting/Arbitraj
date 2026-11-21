//! Файл: notify/reason_compact.rs
//! Что делает: решение 251 (Тигран 10.11: «в алертах всё пишется неразборчиво») — длинные сырые куски в тексте причины
//! человеку не нужны, их место в журнале: hex длиннее HEX_SHORT знаков после «0x» (хеши, данные отката) →
//! «0x1234…abcd» (notify/human.rs::addr); JSON-объект целиком (в фигурных скобках, с кавычками внутри) → «{…}».
//! Логику не меняет: исходный текст остаётся в журнале и в хронологии сделки.
//! Кто вызывает: notify/reason.rs (clean — причина алерта, короткая причина экрана, текст события без заголовка).
//! От чего зависит: notify/human.rs (addr).
//! Настройки: нет — вид текста, не торговые параметры.

/// Сколько hex-знаков после «0x» ещё показывать целиком (16; адрес токена — 40, хеш — 64: сокращаются).
const HEX_SHORT: usize = 16;

/// Сокращает длинный hex и JSON. Принимает: текст. Возвращает: текст. Ошибок нет.
pub fn compact(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '0' && c.get(i + 1) == Some(&'x') {
            let n = c[i + 2..]
                .iter()
                .take_while(|x| x.is_ascii_hexdigit())
                .count();
            if n > HEX_SHORT {
                let hex: String = c[i..i + 2 + n].iter().collect();
                out.push_str(&super::human::addr(&hex));
                i += 2 + n;
                continue;
            }
        }
        if c[i] == '{'
            && let Some(len) = json_len(&c[i..])
        {
            out.push_str("{…}");
            i += len;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Длина JSON-объекта от «{» до парной «}», если внутри есть кавычки (это JSON, а не текст в скобках).
/// Принимает: знаки с «{» в начале. Возвращает: длину в знаках или None. Ошибок нет.
fn json_len(c: &[char]) -> Option<usize> {
    let mut depth = 0usize;
    for (k, ch) in c.iter().enumerate() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return c[..k].contains(&'"').then_some(k + 1);
                }
            }
            _ => {}
        }
    }
    None
}
